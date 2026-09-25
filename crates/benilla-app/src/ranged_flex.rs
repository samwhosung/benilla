//! **The ranged weapon prop's own animation** — the bow's limbs bend, the gun
//! fires its muzzle blast.
//!
//! An equipped ranged weapon is not just a mesh in a hand. The reference keeps its M2 *instance*
//! at `[CGUnit+0xd24]` (written by the ranged-attach helper `0x611e10` @`0x611eca`, populated on
//! any sheath→2) and re-arms **that model's own animation** from two keyframes on the body clip:
//!
//! - **`$BWP`** (BowPull, keyed only in LoadBow 105 @0.434 s / LoadRifle 106 @0.433 s) →
//!   `0x624cc0`: if the prop's model *owns* animation **160 BowPull** (`0x624cee push 0xa0; call
//!   0x711960` — a presence test it bails on), play it on the prop at a rate that fits its
//!   authored length into the body clip's REMAINING time (`0x624e31`'s computed float arg).
//! - **`$BWR`** (BowRelease, keyed only in AttackBow 46 @0.033 s / AttackRifle 49 @0.000 s) →
//!   `0x600159`, which **forks on the weapon family** and arms a different clip for each:
//!   - the body is playing a **bow** clip (`0x5fcfb0`: 46 AttackBow / 105 LoadBow / 109 HoldBow) →
//!     the prop plays **animation 0 (Stand)** (`0x600209`) — the drawn limbs relax;
//!   - the body is playing a **rifle** clip (`0x5fcfd0`: 49 AttackRifle / 106 LoadRifle / 110
//!     HoldRifle — guns AND crossbows) → the prop plays **animation 161 (BowRelease)**
//!     (`0x600273`, `push 0xa1`).
//!
//! **That second arm is the gun's muzzle blast, and it is why this file exists.** A firearm M2
//! authors exactly two sequences — Stand(0) and BowRelease(161) — and hangs its whole flash on the
//! second: `Firearm_2H_Rifle_A_01.m2` carries **seven** particle emitters, each holding a flat rate
//! in every sequence (100/s, 50/s, 40/s, 30/s, 20/s) and keying only its **enabled gate** — shut
//! across Stand, open from the head of 161 for a window of its own (34 ms, 134 ms, 4 × 200 ms,
//! 334 ms: a spark, a flare, the body of the flash, a smoke tail). Bone 2, the parent of all seven
//! emitter bones, is keyed a constant **+90° about Y** in 161 against 0° in Stand — which is what
//! aims the blast down the barrel instead of straight up out of the receiver. 21 of the 22 firearms
//! in the 5875 chain author 161; 25 bows and crossbows author 160 *and* 161; nothing else in the
//! 571-model weapon corpus authors either. Without this arm the emitters build, pool and tick for
//! ever on a sequence whose gate is shut, and a gun shot is silent art.
//!
//! **It is the GATE that is keyed, never the rate** — the track at `def+0x1dc`, outside the ten
//! scalar ones, `u8`-valued, step-interpolated. A consumer that asks "does this
//! emitter emit anything?" of the *rate* sees a non-zero constant in every sequence, for a bank
//! that has never once been switched on; that false negative is why this shipped broken.
//!
//! **The rifle arm is in the same function as the bow arm**, eleven instructions apart, and
//! **exactly four** sites arm `[+0xd24]` — `0x624e31` (160), `0x600209` (0), `0x600273` (161) and
//! `0x60f59d` (0, the reset below). Only **thrown and wand** have no prop re-anim: a third family
//! predicate `0x5fcf90` = {107, 111, 112} exists and the `$BWR` handler never calls it, so those
//! two match neither arm and get no prop animation at all.
//!
//! **Both arms sit under a gate, and it is the projectile queue**. `[CGUnit+0xac]`
//! is not a spell-visual list, as this file first guessed from its readers — it is the queue of
//! `CMissile` nodes waiting for the caster's release event, and `$BWR` reads it (`0x600182`) before
//! draining it (`0x600294` → `0x60c940`, the launcher). So the prop's re-anim and the projectile's
//! launch are two effects of one act: no missile queued, no flex. benilla already held that queue —
//! [`crate::entities::PendingMissiles`], whose own doc named `+0xac` correctly long before this
//! round — and the chain runs this arm ahead of the drain, which is the order the two addresses
//! sit in.
//!
//! What this file does NOT own: the prop's rig and palette rows (the rider lane —
//! [`benilla_world::rig_rider`], given a posed arm by 2281), the emitters' clock (`EmitClock::Host`
//! on the prop root, wired at the attach), or the bowstring chord ([`crate::bowstring`], which now
//! spans the *posed* limb tips because of this).

use bevy::prelude::*;

use benilla_assets::ModelAnimations;

use crate::creature_anim::AnimSoundEvent;

/// `AnimationData.dbc` **160 BowPull** — the prop clip `$BWP` arms (`0x624e2a push 0xa0`).
pub(crate) const BOW_PULL: u16 = 160;
/// `AnimationData.dbc` **161 BowRelease** — the prop clip the RIFLE arm of `$BWR` plays
/// (`0x60026c push 0xa1`). On a firearm this is the muzzle blast; on a bow it is authored and,
/// per the byte fork above, never reached from here.
pub(crate) const BOW_RELEASE: u16 = 161;
/// The model-load bootstrap id — animation **0 Stand**, what the BOW arm of `$BWR` plays
/// (`0x600205 push 0x0`) and what the un-nock's reset re-arms (`0x60f59d`): the drawn bow returns
/// to rest.
const STAND: u16 = 0;

/// `0x5fcfb0` verbatim — the body clips that make the wearer a **bow** shooter.
const BOW_FAMILY: [u16; 3] = [46, 105, 109]; // AttackBow · LoadBow · HoldBow
/// `0x5fcfd0` verbatim — the body clips that make the wearer a **rifle** shooter (gun + crossbow).
const RIFLE_FAMILY: [u16; 3] = [49, 106, 110]; // AttackRifle · LoadRifle · HoldRifle

/// The unit's equipped **ranged weapon prop** — benilla's `[CGUnit+0xd24]`. On the prop root, put
/// there by the equipment attach for the ranged slot when the display's model authors a flex clip
/// (160 or 161); absent for every other held item, which is the reference's own population (nothing
/// else is ever passed to `0x624cc0`/`0x600209`/`0x600273`).
#[derive(Component)]
pub(crate) struct RangedProp {
    /// The unit wearing it — the `$BWP`/`$BWR` keys arrive on *its* timeline, not the prop's.
    pub(crate) owner: Entity,
}

/// Arm the prop's clip from the wearer's `$BWP`/`$BWR` keys.
///
/// Registered in the creature-anim chain immediately after
/// [`crate::creature_anim::drive_nock_latch`], so a key lands the frame it is crossed: the two are
/// the same handler pair in the reference (`$BWP` is `0x624cc0`'s prop arm *then* `0x624b2f`'s ammo
/// attach; `$BWR` is `0x600209`/`0x600273`'s prop arm *then* `0x600299`'s detach), and the prop's
/// pose has to be this frame's before the rider lane composes its palette rows from it.
pub(crate) fn flex_ranged_props(
    mut events: MessageReader<AnimSoundEvent>,
    props: Query<(Entity, &RangedProp)>,
    // Disjoint from `props_mut` by the filter: a unit is never its own ranged prop.
    wearers: Query<(&AnimationPlayer, &ModelAnimations), Without<RangedProp>>,
    mut props_mut: Query<(&mut AnimationPlayer, &ModelAnimations), With<RangedProp>>,
    // The `[+0xac]` gate — read here, drained by `entities::missile::spawn_missiles` on the same
    // key. The creature-anim chain runs `.before(EntityVisualsSet)`, so this read lands ahead of
    // that drain exactly as `0x600182` lands ahead of `0x600294`.
    pending: Res<crate::entities::PendingMissiles>,
) {
    for ev in events.read() {
        let want = match &ev.ident {
            b"$BWP" => Some(BOW_PULL),
            // **`0x60018a je 0x600299`** — no projectile waiting, no prop block at all. The gate
            // is `$BWR`'s alone: `$BWP`'s handler (`0x624cc0`) reads nothing of the kind, and the
            // nock-latch clear above it (`0x60016c`) and the ammo detach below it (`0x600299`) are
            // both outside the skip, which is why `drive_nock_latch` stays ungated.
            b"$BWR" if !pending.releasing(ev.entity) => None,
            b"$BWR" => {
                if BOW_FAMILY.contains(&ev.anim_id) {
                    Some(STAND)
                } else if RIFLE_FAMILY.contains(&ev.anim_id) {
                    Some(BOW_RELEASE)
                } else {
                    // Neither family: the reference falls through to `0x60027a` and touches no
                    // prop at all. Nothing in the shipped character models keys `$BWR` outside
                    // 46/49, so this arm is the reference's own defensive one.
                    None
                }
            }
            _ => continue,
        };
        let Some(want) = want else { continue };
        // The prop is found from the WEARER, exactly as `[+0xd24]` is: one ranged prop per unit.
        let Some((prop, _)) = props.iter().find(|(_, p)| p.owner == ev.entity) else {
            continue;
        };
        let Ok((mut player, anims)) = props_mut.get_mut(prop) else {
            continue;
        };
        // **The two asks are not the same ask, and the asymmetry is real.** `$BWP` is guarded by
        // `0x711960` — the *direct, unsubstituted* "does this model author id X" test — and bails
        // when it fails: that is what confines BowPull to its 25 models, and what a firearm exits
        // on, its whole cycle being the `$BWR` blast. `$BWR`'s arms carry no such guard, and
        // `0x7121a0` does **not** silently no-op on a miss: `0x711bf0` substitutes through the
        // model's own `PlayableAnimationLookup` first, and every weapon model that authors neither
        // flex clip bakes both ids to Stand(0) (measured on the shipped weapon corpus: 571/571
        // resolve). So a rifle-family body holding a model without 161 re-arms Stand, it does not
        // leave the prop alone.
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
        // `$BWP`'s rate: the reference fits BowPull's authored length into the body clip's
        // REMAINING time (`0x624e31`'s computed float), so the draw completes exactly as the Load
        // clip ends rather than finishing early and holding. Read off the wearer's own player at
        // the clip the key fired from — `duration − seek` is the same quantity the reference
        // builds from the block's window. Everything else plays at the literal 1.0 both `$BWR`
        // arms push (`0x3f800000`).
        let speed = if want == BOW_PULL {
            // **`n <= 0` is NO FLEX THAT SHOT, not a flex at 1.0** (`0x624d91 jle`): the
            // reference abandons the whole arm when the body clip has no time left to fill.
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
        // op4 ARMS a model, it does not layer — the prop plays exactly one clip. Without the
        // stop, Bevy leaves the previous node active and `playing_seq` (which the emitters' rate
        // track reads) picks by weight between two equal claims.
        player.stop_all();
        let active = player.play(clip.node);
        active.replay();
        active.set_speed(speed);
        // **The instrument** (`WOW_MOVE_TRACE_TAGS=flex`), on the same clock as the `aev` key that
        // asked for it. Whether a prop armed its clip is otherwise unobservable from outside the
        // renderer — and "the blast did not fire" has three distinct causes (the key never
        // arrived, the fork went the other way, the model owns no such clip) that look identical
        // on screen.
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
        // The authored flags decide the wrap, as everywhere else: a firearm's Stand loops and its
        // BowRelease clamps, a bow's three clips all clamp. A clamped flex holds its last frame
        // until the next key re-arms it, which is the reference's own steady state.
        if clip.looping {
            active.repeat();
        } else {
            active.set_repeat(bevy::animation::RepeatAnimation::Never);
        }
    }
}

/// **The un-nock's reset — `0x60f59d`, and its condition is INVERTED.** Not "re-arms the prop to
/// Stand(0) **after** release": the bytes say **UNLESS** it is releasing:
///
/// ```text
/// 60f578  push -1 ; call 0x712090   ; the prop's CURRENT requested animation id
/// 60f57f  cmp eax,0xa1              ; 161 BowRelease
/// 60f584  je 0x60f5a2               ; …then SKIP the reset
/// ```
///
/// **And the order is the law, not an implementation detail.** The reset lives inside the un-nock
/// `0x60f530`, which `$BWR` itself reaches every shot (`0x600294` → `0x60c940` → `0x60c951`) —
/// *after* the arm at `0x600273`. So the arm runs first and is exactly what that `cmp` reads back;
/// build the two independently and a gun's muzzle blast is cancelled on the frame it starts. The
/// guard exists for precisely this collision.
///
/// benilla reproduces the shape rather than the call graph: the un-nock's observable here is
/// [`NockLatch`] leaving the wearer, which `$BWR` does every shot and every cancel path does too
/// ([`crate::creature_anim::cancel_auto_repeat_local`], leaving the ranged sheath, a weapon
/// change). Removals are visible a frame after the commands that make them apply, by which time a
/// gun's prop is already on its 2 s BowRelease and the guard declines — which is the reference's
/// outcome by the reference's own reasoning.
///
/// What it is actually *for* is the case no release covers: **a cancel while the bow is drawn.**
/// BowPull(160) is a clamp, so a volley stopped mid-pull leaves the limbs bent at full draw for as
/// long as the weapon stays in hand; this is what relaxes them.
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
        // `0x60f57f cmp eax,0xa1` — the prop's current requested id, not its clip time: a
        // BowRelease that has already finished still reads 161 and is still skipped.
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

/// Seconds left in the wearer's playing clip for `anim_id` — the `$BWP` rate's denominator.
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

    /// A model's animation table, with the **two lookups a real M2 carries** built from the clips:
    /// `animation_lookup` (the reference's direct `0x711960` test, which gates `$BWP`) and
    /// `playable_animation_lookup` (the substitution `0x711bf0` runs before every arm — every id
    /// the model does not author resolves to Stand, as the shipped weapon corpus bakes it).
    /// Leaving either empty is what a synthetic fixture gets wrong: `owns()` then answers `false`
    /// for a sequence the model plainly has.
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

    /// A firearm prop: Stand(0) looping + BowRelease(161) clamped, and **no** BowPull — the real
    /// `Firearm_2H_Rifle_A_01.m2` shape (see the real-asset pin below).
    fn gun() -> ModelAnimations {
        anims(vec![
            clip(STAND, 1, 0.333, true),
            clip(BOW_RELEASE, 2, 2.0, false),
        ])
    }

    /// A bow prop: Stand(0), BowPull(160), BowRelease(161) — `Bow_1H_Standard_A_01.m2`'s shape.
    fn bow() -> ModelAnimations {
        anims(vec![
            clip(STAND, 1, 0.033, false),
            clip(BOW_PULL, 2, 1.0, false),
            clip(BOW_RELEASE, 3, 0.166, false),
        ])
    }

    /// Spawn a wearer playing `body` and its prop, run the arm over one key, and report which
    /// node the prop ended up playing (with its speed).
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
        // A projectile queued on the wearer — the `[+0xac]` gate the `$BWR` arms sit under. Every
        // leg but the gate's own test wants it open; `queue_a_shot` is what opens it.
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

    /// **The fork the whole file exists for** (`0x600159`): the SAME `$BWR` key arms a different
    /// clip on the prop depending on which weapon family the body is playing. A gun asked for
    /// BowRelease(161) — its muzzle blast — and a bow for Stand(0), its limbs relaxing.
    ///
    /// The gun leg is the director's report: before this, nothing anywhere played 161, so the
    /// seven emitters that carry the blast sat for ever on Stand's authored rate of zero.
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

    /// **The `[+0xac]` gate** (`0x600182`/`0x60018a`): a `$BWR` with no projectile
    /// waiting to be released arms nothing on the prop. The reference skips the whole block —
    /// prop re-anim and cast-point reposition together — because the flex and the launch are two
    /// effects of one act, and there is no act without a missile to throw.
    ///
    /// Asserted as the difference the gate makes: the SAME key, the same rifle body clip, with and
    /// without a queued shot.
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

    /// `$BWP` arms BowPull — and only on a model that HAS it. The reference makes that a literal
    /// presence test it bails on (`0x624cee push 0xa0; call 0x711960`), and a firearm is exactly
    /// the model it bails for: 21 of the 22 shipped firearms author 161 and none authors 160.
    #[test]
    fn bwp_pulls_only_a_prop_that_owns_bowpull() {
        // BOTH legs supply a body clip with time left in it, so the firearm's `None` can only be
        // the presence test — not the separate `n <= 0` bail the leg below covers.
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

    /// The pull's RATE: the reference stretches BowPull's authored length over the body clip's
    /// REMAINING time (`0x624e31`'s computed float), so the draw completes as the Load clip ends
    /// instead of finishing early and holding. HumanMale LoadBow(105) is 1.0 s and keys `$BWP` at
    /// 0.434 s, so a 1.0 s BowPull has to run at ~1.767×.
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

    /// **The reset's guard** (`0x60f584 je`): the un-nock re-arms Stand **unless** the prop is
    /// releasing. Taken the other way round — "reset after release" — a gun's muzzle blast is
    /// cancelled on the frame it starts, because the reference reaches this reset from the very
    /// `$BWR` that armed 161.
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
        // A gun mid-blast is NOT touched — the whole point of the guard.
        let gun_release = gun().clips[1].node;
        assert_eq!(
            unnock(gun(), BOW_RELEASE),
            Some(gun_release),
            "the reset skips a prop that is releasing — otherwise the blast dies at birth"
        );
    }

    /// **The asset half, on the real chain** — the fact the whole feature rests on, and the one a
    /// data-reading regression would break silently: a firearm's muzzle blast is authored ENTIRELY
    /// on BowRelease(161), with every emitter's rate track keyed to zero on the loader idle.
    ///
    /// This is what makes "the prop never plays 161" invisible rather than obviously broken — the
    /// emitters build, pool and tick, and pour nothing.
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
        // **The discriminator is the GATE, not the rate**. Every one of the seven
        // holds a constant rate in every sequence (100/s, 50/s, 40/s, 30/s, 20/s) and keys only
        // `enabled` — the eleventh track at `def+0x1dc`, `u8`, step: flat 0 across Stand, and
        // `0.000 = 1 → <its own window> = 0` in BowRelease. That is precisely why this defect was
        // silent — a "does this emitter emit anything?" check on the rate says yes, on every
        // sequence, for a bank that has never once been switched on.
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
            // 1.0 s clears every window in the bank — the widest is the 334 ms smoke tail, and
            // the assertion is deliberately the property rather than a per-emitter figure (2286
            // corrected 2281's "0.2 s", which was the modal window read as the only one).
            assert!(
                !em.timing.emitting(Some(1), 1.0, 0.0),
                "emitter {i} is over well before BowRelease's 2 s band ends"
            );
        }
    }

    /// The other half of the corpus law, so the `flexes` gate at the attach is pinned to data and
    /// not to a guess: bows and crossbows author BOTH flex clips, an ordinary melee weapon
    /// authors NEITHER (and so never gets a pose, a player or a hosted emitter clock).
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
