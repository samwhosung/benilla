//! Headless integration tests for [`super::drive_animations`] — the full driver system run in a
//! minimal app on synthetic units, exercising the cross-frame composition the pure-fn tests in
//! `select::tests` can't reach. First tenant: the caster staff-stow chain (
//! the stationary cast-hold gait pin feeding the per-animation sheath reconcile), pinned because
//! vmangos hard-sets every creature's sheath byte to melee at spawn ("creatures always have melee
//! weapon ready", `Creature.cpp`), so the cast-hold clip's WeaponFlags `&4` is the ONLY signal
//! that ever stows a caster NPC's weapon.

use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::transition::AnimationTransitions;
use bevy::prelude::*;

use benilla_assets::{AnimClip, ModelAnimations};
use benilla_formats::{AnimDataCatalog, AnimEntry};

use super::super::{
    move_flags, AnimData, AnimDriver, CastHold, EmoteAnim, Engaged, MovementState, SheathRequest,
    SheathSwapMessage, SwingMessage, Wielded, WoundAnim,
};
use super::drive_animations;
use crate::names::type_flags::{DO_NOT_PLAY_WOUND_ANIM, MORE_AUDIBLE, NO_FACTION_TOOLTIP};
use crate::net::NetCommands;
use benilla_protocol::ObjectFields;

/// A hand holding something that is **not a weapon** — a torch, a held-in-off-hand trinket.
///
/// Several fixtures below want a unit whose auto-attack swing is AttackUnarmed(16) *and* whose
/// spell-kit clip is the armed Special1H(57). Since decision 1863 that combination needs a
/// non-EMPTY hand: the reference's play-time substitution (`0x5fe2f0` @ `0x5fe3cc`, and our
/// [`super::super::select::unarmed_special`]) keys on `GetWeapon(slot, 0)` being **NULL**, not on
/// the item failing to be a weapon — so a held non-weapon keeps the armed special while the swing
/// table still sends it to 16. An empty-handed unit plays SpecialUnarmed(118), which has its own
/// test.
fn holding_a_non_weapon() -> Wielded {
    Wielded {
        main: Some((15, 0)), // ItemClass 15 = miscellaneous
        ..Default::default()
    }
}

fn clip(anim_id: u16, node: u32, looping: bool) -> AnimClip {
    AnimClip {
        anim_id,
        seq_index: 0,
        node: AnimationNodeIndex::new(node as usize),
        looping,
        duration: 1.0,
        move_speed: 0.0,
        blend_time: 0.15,
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

/// A staff-caster's model: Stand, Run, the staff Ready idle, and the precast hold clip (with a
/// masked upper-body variant, so the committed-move route has its overlay destination).
fn caster_model() -> ModelAnimations {
    let mut hold = clip(51, 3, true); // ReadySpellDirected — the precast hold
    hold.upper_node = Some(AnimationNodeIndex::new(5));
    ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),  // Stand
            clip(28, 2, true), // Ready2HL — the staff-class Ready idle
            hold,
            clip(5, 4, true), // Run
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    }
}

/// The real 5875 rows this chain rests on (decode-verified in `anim_data::tests`):
/// ReadySpellDirected carries the force-stow WeaponFlags `&4`, Ready2HL and Attack1H the
/// force-draw `&0x20`.
fn catalog() -> AnimData {
    AnimData(AnimDataCatalog::from_rows([
        (
            0,
            AnimEntry {
                weapon_flags: 0,
                fallback: 0,
            },
        ),
        (
            17,
            AnimEntry {
                weapon_flags: 0x20,
                fallback: 0,
            },
        ),
        (
            28,
            AnimEntry {
                weapon_flags: 0x20,
                fallback: 0,
            },
        ),
        (
            51,
            AnimEntry {
                weapon_flags: 4,
                fallback: 52,
            },
        ),
    ]))
}

/// What a bare `app.update()` advances the harness clock by — small and nonzero, which is the
/// regime every assertion in this file was written against (a real frame used to land here by
/// accident). Zero is NOT equivalent: a frame with no delta never ticks a transition at all, so
/// clips the tests expect to find mid-fade are never started.
const FRAME_STEP: std::time::Duration = std::time::Duration::from_millis(1);

/// The delta [`step_clock`] will use for the next frame, when a test asked for more than
/// [`FRAME_STEP`]. It has to travel as a resource rather than a plain pre-`update()` call on
/// `Time`, because `Time::advance_by` *sets* the frame's delta rather than accumulating it — so a
/// value written before `update()` is simply overwritten by the in-frame step.
#[derive(Resource, Default)]
struct NextStep(Option<std::time::Duration>);

/// The harness's whole clock, in place of `TimePlugin`'s real one (see [`app`]): one frame of
/// [`FRAME_STEP`], or of whatever [`advance`] asked for.
fn step_clock(mut time: ResMut<Time>, mut next: ResMut<NextStep>) {
    time.advance_by(next.0.take().unwrap_or(FRAME_STEP));
}

/// Run one frame whose delta is exactly `ms` — the only way time moves further than
/// [`FRAME_STEP`] (see [`app`]). Replaces `thread::sleep` + `update()`: the same intent, but the
/// delta is the number written here rather than however long the machine happened to take.
fn advance(app: &mut App, ms: u64) {
    app.world_mut().resource_mut::<NextStep>().0 = Some(std::time::Duration::from_millis(ms));
    app.update();
}

fn app() -> App {
    let mut app = App::new();
    // The client's ONE `rand()` stream (2301) — the play's variation and replay rolls.
    app.init_resource::<benilla_assets::AnimRng>();
    // Asset + animation plugins so tests with REAL clip assets (the watchdog test) get Bevy's
    // `advance_animations` ticking completions; units without a graph handle are skipped by it,
    // so the asset-less tenants are unaffected.
    //
    // **`TimePlugin` is deliberately disabled, and the clock is ours** ([`step_clock`]). With it,
    // `Time`'s delta is the REAL gap between two `app.update()` calls, and this file is full of
    // assertions that a fade or clip is still mid-flight. A stalled frame — parallel `cargo test`
    // on a loaded machine, a debugger, a cold page — then runs the whole fade out in one delta and
    // the clip is gone from the player, which is a coin flip rather than a test: it red-lit main's
    // gates on 2026-08-06 in `the_swim_relatch_holds_the_kick_but_a_ground_cut_freezes_it`, and
    // injecting a 400 ms stall before that test's ground cut reproduces the failure exactly. The
    // tests that DO need time to pass used to buy it with `thread::sleep`, which is the same coin
    // flip pointed the other way; they now say how far the clock moves, through [`advance`].
    //
    // The step is small-but-nonzero rather than zero on purpose — see [`FRAME_STEP`].
    app.add_plugins((
        MinimalPlugins.build().disable::<bevy::time::TimePlugin>(),
        AssetPlugin::default(),
        bevy::animation::AnimationPlugin,
    ));
    app.init_resource::<Time>();
    app.init_resource::<NextStep>();
    app.add_systems(bevy::app::First, step_clock);
    app.add_message::<SwingMessage>()
        .add_message::<crate::creature_anim::SwingImpact>()
        .add_message::<crate::creature_anim::DefenseAnim>()
        .add_message::<crate::creature_anim::SwingSlowdown>()
        .add_message::<EmoteAnim>()
        .add_message::<super::super::BaseAnimRecompute>()
        .add_message::<WoundAnim>()
        .add_message::<SheathRequest>()
        .add_message::<SheathSwapMessage>();
    // A dead-letter net channel: the driver's `let _ = send(...)` tolerates the dropped receiver,
    // and no test unit is the self player anyway.
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    app.init_resource::<crate::names::NameCache>();
    app.insert_resource(catalog());
    app.add_systems(Update, drive_animations);
    app
}

/// The caster-NPC staff chain, end to end through the real system: engaged Ready draws (reconcile
/// rule 4), the stationary cast hold's gait pin stows (rule 1 — `&4` outranks engaged), the hold's
/// removal re-draws. The director's report ("caster NPC still holds their staff while casting")
/// is exactly this middle assertion.
#[test]
fn stationary_cast_hold_stows_an_engaged_casters_weapon() {
    let mut app = app();
    let unit = app
        .world_mut()
        .spawn((
            caster_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Engaged(0),
            Wielded {
                main: Some((2, 0xa)), // class 2 subclass 10: a staff
                off: None,
                ranged: None,
                main_sheath: 2,
                off_sheath: 0,
                ..Default::default()
            },
        ))
        .id();
    let sheath = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .sheath_state()
    };

    // Engaged, stationary, no cast: the Ready idle forces melee-drawn.
    app.update();
    assert_eq!(sheath(&app), Some(1), "engaged Ready idle draws");

    // SMSG_SPELL_START landed (the router inserted the precast hold): the stationary pin plays
    // ReadySpellDirected full-body in the gait slot, and its WeaponFlags `&4` force-stows.
    app.world_mut().entity_mut(unit).insert(CastHold {
        ranged: false,
        anim_id: 51,
        spell_id: 20793,
    });
    app.update();
    assert_eq!(sheath(&app), Some(0), "the cast hold stows the staff");

    // GO (the router removed the hold): the engaged Ready re-takes the slot and re-draws.
    app.world_mut().entity_mut(unit).remove::<CastHold>();
    app.update();
    assert_eq!(sheath(&app), Some(1), "drawn again once the cast resolves");
}

/// The committed-move route: the hold loops masked on the torso over the gait, and its stow must
/// HOLD between plays — the reconcile is edge-triggered like the client's (`0x5fdf80` runs only
/// inside `PlayAnimation`), so the base track's flags-less Run never
/// re-draws mid-hold on the frames where nothing plays. This was the caster staff bug's shape:
/// the per-frame base-track re-assert yanked the weapon back out one frame after the retake.
#[test]
fn moving_cast_hold_keeps_its_stow_between_plays() {
    let mut app = app();
    let unit = app
        .world_mut()
        .spawn((
            caster_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Engaged(0),
            Wielded {
                main: Some((2, 0xa)),
                off: None,
                ranged: None,
                main_sheath: 2,
                off_sheath: 0,
                ..Default::default()
            },
            MovementState {
                speed: 7.0,
                flags: move_flags::FORWARD,
                ..Default::default()
            },
        ))
        .id();
    let sheath = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .sheath_state()
    };

    // Engaged and running: the flags-less Run gait plays, the engaged re-assert draws.
    app.update();
    assert_eq!(sheath(&app), Some(1), "engaged runner draws");

    // The precast lands mid-move: the hold takes the masked overlay route — its retake is a play,
    // so the reconcile stows.
    app.world_mut().entity_mut(unit).insert(CastHold {
        ranged: false,
        anim_id: 51,
        spell_id: 20793,
    });
    app.update();
    assert_eq!(sheath(&app), Some(0), "the masked hold's retake stows");

    // Frames where nothing plays (the gait loop wraps, the hold loops): the committed state holds.
    for _ in 0..3 {
        app.update();
        assert_eq!(sheath(&app), Some(0), "no play — the stow persists");
    }

    // GO while still running: the hold drops, but the base Run keeps looping — still no play, so
    // the staff stays stowed (the client re-draws only at the next play).
    app.world_mut().entity_mut(unit).remove::<CastHold>();
    app.update();
    assert_eq!(sheath(&app), Some(0), "released mid-run — no play yet");

    // The creature stops: the engaged Ready idle plays, and that play's reconcile re-draws.
    app.world_mut().entity_mut(unit).remove::<MovementState>();
    app.update();
    assert_eq!(sheath(&app), Some(1), "the stop's Ready play re-draws");
}

/// A fidgeter's model, on **real Bevy clip assets** (see [`spawn_vendor`] for why: this tenant now
/// runs to a window completion, and `completions()` only ticks for a real graph): Stand as a
/// two-variation chain — a zero-frequency head plus a max-frequency "look around" variation, so
/// the first `_rand` roll (38, from the LCG's zero seed) deterministically lands on the variation —
/// and a ShuffleLeft for the turn latch. Stand's span is long enough that its own window never
/// completes inside the test; the shuffle's is short so the test does not have to run a real
/// half-second to reach the release.
///
/// Returns the entity and the graph nodes in clip order: Stand-head, Stand-look, ShuffleLeft.
fn spawn_fidgeter(app: &mut App) -> (Entity, Vec<AnimationNodeIndex>) {
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    const STAND_SPAN: f32 = 4.0;
    const SHUFFLE_SPAN: f32 = 0.1;
    let handles: Vec<_> = [STAND_SPAN, STAND_SPAN, SHUFFLE_SPAN]
        .iter()
        .map(|d| {
            let mut c = AnimationClip::default();
            c.set_duration(*d);
            app.world_mut()
                .resource_mut::<Assets<AnimationClip>>()
                .add(c)
        })
        .collect();
    let (graph, nodes) = AnimationGraph::from_clips(handles);
    let graph_handle = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);
    let seq = |id: u16, node, duration: f32, frequency: u16| AnimClip {
        node,
        duration,
        blend_time: 0.0,
        frequency,
        replay: (0, 0),
        ..clip(id, 0, true)
    };
    let anims = ModelAnimations {
        graph: graph_handle.clone(),
        clips: vec![
            seq(0, nodes[0], STAND_SPAN, 0),     // Stand — the head variation
            seq(0, nodes[1], STAND_SPAN, 32767), // Stand — the rare look-around variation
            seq(11, nodes[2], SHUFFLE_SPAN, 0),  // ShuffleLeft
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            anims,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
        ))
        .id();
    (unit, nodes)
}

/// The emergent idle fidget: a RELAXED base
/// arm rolls its variation (the client's `variationIdx = −1`), an engaged one is forced to the
/// deterministic head, and the idle re-face turn-shuffle ([`crate::net::FacingStep`]) drives the
/// Shuffle↔Stand churn whose every return to Stand re-rolls.
///
/// **What the return is triggered BY changed in 1655**, and that is the middle of this test. 0123
/// had the shuffle released the frame the yaw ease settled; the reference cannot do that
/// (`0x607ed0`'s tail refuses to recompute a shuffle with no turn bit set — `0x5fce30`'s gate),
/// and releases it a clip window later through the completion callback instead. The churn 0123
/// describes is intact and the re-roll still rides it; it simply happens at the window boundary,
/// which is also why the shuffle is long enough to be seen.
#[test]
fn relaxed_base_arms_roll_variations_and_the_shuffle_drives_them() {
    let mut app = app();
    let (unit, nodes) = spawn_fidgeter(&mut app);
    let active = |app: &App, node: AnimationNodeIndex| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(node)
            .is_some()
    };
    let gait = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;

    // The first (relaxed) Stand arm rolls: the weighted walk lands on the look-around variation.
    app.update();
    assert_eq!(gait(&app), Some(0));
    assert!(active(&app, nodes[1]), "the rolled variation is what armed");
    assert!(!active(&app, nodes[0]), "not the head");

    // The idle re-face steps its yaw: the turn latch routes the gait to the foot-shuffle.
    app.world_mut()
        .entity_mut(unit)
        .insert(crate::net::FacingStep(0.3));
    app.update();
    assert_eq!(gait(&app), Some(11), "stepping yaw → ShuffleLeft");

    // The ease settles — and the feet keep going, because the settle is not what releases them.
    app.world_mut()
        .entity_mut(unit)
        .remove::<crate::net::FacingStep>();
    advance(&mut app, 25);
    assert_eq!(
        gait(&app),
        Some(11),
        "the settle does not release the shuffle (1655)"
    );

    // Its window completes: THAT is the release, and the re-selection it runs re-arms Stand with a
    // fresh roll — the fidget.
    for _ in 0..6 {
        advance(&mut app, 25);
    }
    assert_eq!(gait(&app), Some(0), "the completed window → back to Stand");
    assert!(
        active(&app, nodes[0]) || active(&app, nodes[1]),
        "some Stand variation re-armed"
    );
}

/// The combat carve-out: an engaged unit's base arms keep the deterministic head — fighters
/// never fidget (the client's `0x5fdba0` re-zero gate).
#[test]
fn engaged_base_arms_keep_the_head_variation() {
    let mut app = app();
    let (unit, nodes) = spawn_fidgeter(&mut app);
    app.world_mut().entity_mut(unit).insert(Engaged(0));
    app.update();
    let player = app.world().entity(unit).get::<AnimationPlayer>().unwrap();
    // Engaged with no weapon: the Ready pick resolves down to Stand — armed as the HEAD.
    assert!(player.animation(nodes[0]).is_some(), "the head variation");
    assert!(
        player.animation(nodes[1]).is_none(),
        "no roll while engaged"
    );
}

/// The GnollCaster case (the director's ref falsification of the resolved-id
/// reading): a model with NO spell animations at all falls back to a flags-less Stand for
/// *playback*, but the sheath reconcile tests the **requested** id — ReadySpellDirected's own
/// force-stow row — so the staff still leaves the hand for the whole windup, exactly like the
/// ref's Redridge Mystic.
#[test]
fn cast_hold_stows_even_when_the_model_lacks_the_spell_anims() {
    let mut app = app();
    // A gnoll-shaped model: Stand and a Ready idle only — no 51/53 anywhere — with the real
    // gnoll's baked lookup shape (row 51 → Stand), so playback of the hold genuinely lands on
    // the flags-less Stand clip and only the requested id's own row can stow.
    let mut lookup = vec![
        benilla_formats::PlayableAnim {
            resolved_id: 0,
            dir_flags: 0,
        };
        64
    ];
    lookup[26].resolved_id = 26;
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![clip(0, 1, true), clip(26, 2, true)],
        hand_close: [None, None],
        playable_animation_lookup: lookup,
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Engaged(0),
            Wielded {
                main: Some((2, 0xa)),
                off: None,
                ranged: None,
                main_sheath: 2,
                off_sheath: 0,
                ..Default::default()
            },
        ))
        .id();
    let sheath = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .sheath_state()
    };

    app.update();
    assert_eq!(sheath(&app), Some(1), "engaged Ready draws");

    // The precast hold requests 51; playback resolves to Stand (the model has nothing better),
    // but 51's own WeaponFlags `&4` still force the stow.
    app.world_mut().entity_mut(unit).insert(CastHold {
        ranged: false,
        anim_id: 51,
        spell_id: 20792,
    });
    app.update();
    assert_eq!(
        sheath(&app),
        Some(0),
        "the requested hold id stows regardless of the playback fallback"
    );

    app.world_mut().entity_mut(unit).remove::<CastHold>();
    app.update();
    assert_eq!(sheath(&app), Some(1), "re-drawn once the cast resolves");
}

/// A spell-side flinch rides the wound **secondary slot**, never the one-shot route — the
/// client's `0x60ea70(severity = 0)` from the kit player's 8–10 branch, the harmful instant
/// impact, or the missile arrival: the [`WoundAnim`] edge arms the decaying
/// overlay and the base track keeps playing untouched underneath (routing it as a one-shot would
/// replace the base — the exact mistake decision 0111 falsified for melee).
#[test]
fn spell_impact_wound_rides_the_secondary_slot() {
    let mut app = app();
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),  // Stand
            clip(8, 2, false), // StandWound — the unengaged victim's severity-0 pick
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
        ))
        .id();

    app.update(); // settle: Stand holds the gait slot
    fn drv(app: &App, unit: Entity) -> &AnimDriver {
        app.world().entity(unit).get::<AnimDriver>().unwrap()
    }
    assert!(drv(&app, unit).wound.is_none());
    let gait_before = drv(&app, unit).gait;

    app.world_mut().write_message(WoundAnim { entity: unit });
    app.update();
    assert!(
        drv(&app, unit).wound.is_some(),
        "the spell flinch armed the secondary slot"
    );
    assert_eq!(
        drv(&app, unit).gait,
        gait_before,
        "the base track is untouched — a decaying overlay, not a replace"
    );
}

/// The spell flinch is the client's **severity-0** wound call: it never carries
/// the kit's own id, so the clip is CombatWound(9) on an engaged victim and StandWound(8) on an
/// unengaged one — `0x60ea70`'s `(severity, engaged)` pick, the same as a non-crit melee hit.
#[test]
fn spell_flinch_picks_the_wound_by_engagement() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, 1, true),   // Stand
                clip(8, 2, false),  // StandWound
                clip(9, 3, false),  // CombatWound
                clip(10, 4, false), // CombatCritical — a spell flinch must never land here
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }
    let mut app = app();
    let engaged = app
        .world_mut()
        .spawn((
            model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Engaged(0),
        ))
        .id();
    let idle = app
        .world_mut()
        .spawn((
            model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
        ))
        .id();
    app.update(); // settle both bases
    app.world_mut().write_message(WoundAnim { entity: engaged });
    app.world_mut().write_message(WoundAnim { entity: idle });
    app.update();
    let node = |e: Entity| {
        app.world()
            .entity(e)
            .get::<AnimDriver>()
            .unwrap()
            .wound
            .map(|w| w.node.index())
    };
    assert_eq!(node(engaged), Some(3), "engaged: CombatWound(9)");
    assert_eq!(node(idle), Some(2), "unengaged: StandWound(8)");
}

/// The wound trigger's fourth entry gate (`0x60eaac`–`0x60eac8`): a CharProc-11
/// rate-override node on
/// the unit — what a freeze aura (Ice Block, Freezing Trap, petrify, web wrap) leaves attached —
/// refuses every flinch for its life. The gate is the node's *presence*: kit 3071's rate of 1.0
/// (no freeze at all) closes it exactly like the family's 0.0. A unit without the node flinches.
#[test]
fn a_rate_override_node_refuses_the_wound() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![clip(0, 1, true), clip(8, 2, false)],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }
    let mut app = app();
    let spawn = |app: &mut App, nodes: Option<crate::aura_visual::AuraNodes>| {
        let mut e = app.world_mut().spawn((
            model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
        ));
        if let Some(n) = nodes {
            e.insert(n);
        }
        e.id()
    };
    let frozen = spawn(
        &mut app,
        Some(crate::aura_visual::AuraNodes::with_rate_node_for_tests(
            11958, 0.0,
        )),
    );
    let held_at_one = spawn(
        &mut app,
        Some(crate::aura_visual::AuraNodes::with_rate_node_for_tests(
            3071, 1.0,
        )),
    );
    let free = spawn(&mut app, None);
    app.update();
    for e in [frozen, held_at_one, free] {
        app.world_mut().write_message(WoundAnim { entity: e });
    }
    app.update();
    let wounded = |e: Entity| {
        app.world()
            .entity(e)
            .get::<AnimDriver>()
            .unwrap()
            .wound
            .is_some()
    };
    assert!(
        !wounded(frozen),
        "Ice Block's rate-0 node refuses the flinch"
    );
    assert!(
        !wounded(held_at_one),
        "the gate is the node, not the rate: a 1.0 node refuses too"
    );
    assert!(wounded(free), "no node: the flinch lays as before");
}

/// The whiff slow-down touches SWING anims only (decision 0279's scoping): a spell kit's
/// full-body special (Special1H 57) rides the same `Mode::Swing` slot, and a concurrent
/// auto-attack miss must not drag it to half speed — the director's "the Eviscerate spin
/// drags". A real swing keeps the verified 0.5 write.
#[test]
fn whiff_slowdown_spares_a_non_swing_oneshot() {
    let mut app = app();
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(57, 2, false), // Special1H — Eviscerate's kit anim
            clip(16, 3, false), // AttackUnarmed — the bare-hands swing
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let spinner = app
        .world_mut()
        .spawn((
            model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            holding_a_non_weapon(),
        ))
        .id();
    let swinger = app
        .world_mut()
        .spawn((
            model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            holding_a_non_weapon(),
        ))
        .id();
    app.update(); // settle: Stand holds both gait slots

    // The kit anim plays as a full-body one-shot; the swing as its own.
    app.world_mut().write_message(EmoteAnim {
        entity: spinner,
        anim_id: 57,
        seq: 1,
    });
    app.world_mut().write_message(SwingMessage {
        attacker: swinger,
        victim: None,
        hit_info: 0,
        victim_state: 2, // dodge — the whiff class
        damage: 0,
        displayed: true,
        seq: 2,
    });
    app.update();
    // Both whiff the same frame the one-shots are in flight.
    app.world_mut()
        .write_message(crate::creature_anim::SwingSlowdown(spinner));
    app.world_mut()
        .write_message(crate::creature_anim::SwingSlowdown(swinger));
    app.update();

    let speed = |app: &App, unit: Entity, node: u32| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(AnimationNodeIndex::new(node as usize))
            .expect("one-shot in flight")
            .speed()
    };
    assert_eq!(
        speed(&app, spinner, 2),
        1.0,
        "the special is not a swing — the whiff must not drag it"
    );
    assert_eq!(
        speed(&app, swinger, 3),
        0.5,
        "the real swing keeps the verified half-speed follow-through"
    );
}

/// A same-frame swing/kit-anim collision runs the client's COMBAT FAST-PATH (
/// `0x5fe43c`–`0x5fe48b`): the requests replay in [`PlaySeq`] wire order, the FIRST
/// arms, and the second — combat over combat — does NOT overwrite it: the armed clip doubles
/// to 2× and the second parks in the deferred cache. Both wire orders keep the first arrival on
/// the body. The director's ref ground truth this pins: the Eviscerate spin survives the
/// auto-swings its cast triggers — sped up, never cut.
#[test]
fn same_frame_collision_fast_paths_the_second_combat_clip() {
    let mut app = app();
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(57, 2, false), // Special1H — Eviscerate's kit anim
            clip(16, 3, false), // the bare-hands swing
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let mut unit = || {
        app.world_mut()
            .spawn((
                model(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimDriver::default(),
                holding_a_non_weapon(),
            ))
            .id()
    };
    let spin_last = unit();
    let swing_last = unit();
    app.update(); // settle: Stand holds both gait slots

    // spin_last: the kit anim arrived after the swing on the wire — the spin must win.
    app.world_mut().write_message(SwingMessage {
        attacker: spin_last,
        victim: None,
        hit_info: 0x2,
        victim_state: 1,
        damage: 21,
        displayed: true,
        seq: 1,
    });
    app.world_mut().write_message(EmoteAnim {
        entity: spin_last,
        anim_id: 57,
        seq: 2,
    });
    // swing_last: the wire order reversed — the swing must win.
    app.world_mut().write_message(EmoteAnim {
        entity: swing_last,
        anim_id: 57,
        seq: 3,
    });
    app.world_mut().write_message(SwingMessage {
        attacker: swing_last,
        victim: None,
        hit_info: 0x2,
        victim_state: 1,
        damage: 21,
        displayed: true,
        seq: 4,
    });
    app.update();

    fn drv(app: &App, unit: Entity) -> &AnimDriver {
        app.world().entity(unit).get::<AnimDriver>().unwrap()
    }
    let speed = |app: &App, unit: Entity, node: u32| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(AnimationNodeIndex::new(node as usize))
            .expect("armed clip in flight")
            .speed()
    };
    // Swing first on the wire: the swing arms, the spin fast-paths — the swing doubles and the
    // spin parks (it plays when the swing ends; the ref's swing-first batch shows exactly this).
    assert_eq!(
        drv(&app, spin_last).mode,
        super::super::select::Mode::Swing {
            id: 16,
            under: None,
        },
        "the first arrival holds the body"
    );
    assert_eq!(drv(&app, spin_last).deferred, Some(57), "the spin parks");
    assert_eq!(speed(&app, spin_last, 3), 2.0, "the armed swing doubles");
    // Spin first on the wire (the trace's t=106 batch): the spin arms and SURVIVES the swing —
    // doubled, with the swing parked behind it. The old last-call-wins model ate the spin here.
    assert_eq!(
        drv(&app, swing_last).mode,
        super::super::select::Mode::Swing {
            id: 57,
            under: None,
        },
        "the spin holds the body through the later swing"
    );
    assert_eq!(drv(&app, swing_last).deferred, Some(16), "the swing parks");
    assert_eq!(speed(&app, swing_last, 2), 2.0, "the spin doubles");
}

/// The deferred-cache consumer (the client's `+0xd60` read at the base recompute): the moment no
/// one-shot is live, the parked combat clip plays — the swing the spin deferred fires once the
/// spin ends, at normal rate. Hand-sets the cache with the body idle (the state the instant the
/// spin finished) because the headless harness never advances clips to completion.
#[test]
fn deferred_combat_clip_plays_once_the_body_frees() {
    let mut app = app();
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(16, 3, false), // the bare-hands swing
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
        ))
        .id();
    app.update(); // settle: Stand holds the gait slot
    app.world_mut()
        .entity_mut(unit)
        .get_mut::<AnimDriver>()
        .unwrap()
        .deferred = Some(16);
    app.update();
    let drv = app.world().entity(unit).get::<AnimDriver>().unwrap();
    assert_eq!(
        drv.mode,
        super::super::select::Mode::Swing {
            id: 16,
            under: None,
        },
        "the parked swing armed"
    );
    assert_eq!(drv.deferred, None, "the cache is consumed");
    let speed = app
        .world()
        .entity(unit)
        .get::<AnimationPlayer>()
        .unwrap()
        .animation(AnimationNodeIndex::new(3))
        .expect("swing in flight")
        .speed();
    assert_eq!(speed, 1.0, "a consumed clip plays at normal rate");
}

/// The post-shot leg slide (director-observed vs ref): a one-shot that routed FULL-BODY while
/// standing must yield to the gait the instant the movement flags change — the client's
/// locomotion re-arm lands on the change and blindly overwrites bone 0 (the decision 0280
/// re-arm; `Mode::Land` re-picks on the same edge). Holding the clip out slides the runner
/// over the ground on straight legs. The edge is the trigger, not the level: with the flags
/// steady the clip plays out (third assertion, via the boneless masked fallback's moving entry).
#[test]
fn a_movement_flag_change_cuts_a_full_body_oneshot_immediately() {
    let mut app = app();
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(5, 2, true),   // Run
            clip(16, 3, false), // the bare-hands swing (1.0 s — far from finished)
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            MovementState::default(),
        ))
        .id();
    app.update(); // settle: Stand
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;

    // A standing swing routes full-body onto the base track.
    app.world_mut().write_message(SwingMessage {
        attacker: unit,
        victim: None,
        hit_info: 0x2,
        victim_state: 1,
        damage: 21,
        displayed: true,
        seq: 1,
    });
    app.update();
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Swing {
            id: 16,
            under: None,
        },
        "standing swing holds the base track"
    );

    // The player starts running one frame later: the flag change must re-pick the gait NOW,
    // not when the 1.0 s clip finishes.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD,
        ..Default::default()
    });
    app.update();
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Gait,
        "the movement-flag change cuts the swing to the gait immediately"
    );

    // Steady flags: a fresh standing swing plays out (still Swing on the very next frame).
    app.world_mut()
        .entity_mut(unit)
        .insert(MovementState::default());
    app.update(); // the return to standing re-picks the idle
    app.world_mut().write_message(SwingMessage {
        attacker: unit,
        victim: None,
        hit_info: 0x2,
        victim_state: 1,
        damage: 21,
        displayed: true,
        seq: 2,
    });
    app.update();
    app.update();
    assert!(
        matches!(mode(&app), super::super::select::Mode::Swing { id: 16, .. }),
        "steady flags let the clip play out"
    );
}

/// A stationary caster mouselook-turning: the chase-step TURN flag flickers at mouse-event
/// cadence (set on delta frames, clear on quiet ones — `drive_body_heading`'s fold), but the
/// client's cast pin tests `[9e8] & 0x20000f` — translation + swim, NEVER the turn bits
/// (`0x5fde80`, `move_flags::CAST_PIN_MOVE`) — so the full-body hold stays
/// pinned through the flap. Routing this through the one-shot mask (`0x20003f`) instead churned
/// the gait hold↔Shuffle on every mouse-delta frame — the frostbolt right-drag jitter.
#[test]
fn turning_in_place_never_unpins_the_stationary_cast_hold() {
    let mut app = app();
    let mut model = caster_model();
    model.clips.push(clip(11, 7, true)); // ShuffleLeft — the churn destination the bug routed to
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            MovementState::default(),
            CastHold {
                ranged: false,
                anim_id: 51,
                spell_id: 116,
            },
        ))
        .id();
    let playing = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .playing()
    };

    app.update();
    assert_eq!(playing(&app), (Some(51), None), "stationary: the hold pins");

    // Flap the chase-step TURN flag across frames (mouse delta / quiet / delta …).
    for frame in 0..6u32 {
        let flags = if frame % 2 == 0 {
            move_flags::TURN_LEFT
        } else {
            0
        };
        app.world_mut().entity_mut(unit).insert(MovementState {
            flags,
            ..Default::default()
        });
        app.update();
        assert_eq!(
            playing(&app),
            (Some(51), None),
            "turn flap frame {frame}: pinned full-body, no overlay"
        );
    }

    // Real translation still demotes: the gait leaves the pin and the masked hold takes over.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    let (base, overlay) = playing(&app);
    assert_ne!(base, Some(51), "a translating caster leaves the pin");
    assert_eq!(overlay, Some(51), "…and loops the hold masked on the torso");
}

/// The swim re-latch does NOT cut the hop's kick (director-corrected — amends
/// 0503's swim arm): JumpStart PLAYS OUT over the re-latch, the swim gait waiting at its end.
/// A GROUND cut (landing on a bank) still cuts immediately with 0503's pose-snapshot freeze.
#[test]
fn the_swim_relatch_holds_the_kick_but_a_ground_cut_freezes_it() {
    let mut app = app();
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(41, 2, true),  // SwimIdle
            clip(42, 3, true),  // Swim
            clip(37, 4, false), // JumpStart — the kick (833 ms real; the test never advances it)
            clip(38, 5, true),  // Jump hang
            clip(39, 6, false), // JumpEnd
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            MovementState::default(),
        ))
        .id();
    app.update(); // settle: Stand
    let drv = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;

    // The dolphin hop launches: FALLING with an upward seed → the JumpStart bracket enters.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING | move_flags::FORWARD,
        vertical_speed: 9.0,
        speed: 4.7,
        ..Default::default()
    });
    app.update();
    assert_eq!(
        drv(&app),
        super::super::select::Mode::Entering(super::super::select::Special::Jump),
        "the upward launch enters the JumpStart bracket"
    );

    // Swim re-latches ~0.24 s later, mid-kick: the kick is HELD — no cut, no gait yet — and
    // keeps PLAYING (speed 1, not 0503's frozen ground-cut).
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::SWIMMING | move_flags::FORWARD,
        speed: 4.7,
        ..Default::default()
    });
    app.update();
    app.update();
    assert_eq!(
        drv(&app),
        super::super::select::Mode::Entering(super::super::select::Special::Jump),
        "the re-latch holds the kick (0517) — the swim gait waits for its end"
    );
    let player = app.world().entity(unit).get::<AnimationPlayer>().unwrap();
    let kick = player
        .animation(AnimationNodeIndex::new(4))
        .expect("the held JumpStart is still the armed clip");
    assert_eq!(
        kick.speed(),
        1.0,
        "held, not frozen — the kick keeps playing"
    );

    // A GROUND cut is unchanged: a fresh hop that lands on a bank (flags drop to grounded,
    // no SWIMMING) cuts immediately — Land pick + the 0503 snapshot-freeze on the kick.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: 0,
        ..Default::default()
    });
    app.update();
    assert_eq!(
        drv(&app),
        super::super::select::Mode::Land { id: 39, flags: 0 },
        "a stopped ground landing picks JumpEnd"
    );
    let player = app.world().entity(unit).get::<AnimationPlayer>().unwrap();
    let kick = player
        .animation(AnimationNodeIndex::new(4))
        .expect("the cut JumpStart still fades under the transition");
    assert_eq!(
        kick.speed(),
        0.0,
        "the ground cut is FROZEN mid-pose (0503)"
    );
}

/// The loot kneel, REMOTE half (the `0x5fd8b0` chain's loot leg → Loot 50):
/// `UNIT_FLAG_LOOTING` (`UNIT_FIELD_FLAGS` = field 46, bit 0x400 — up exactly while the unit's
/// corpse-loot window is open) holds the authored-clamp kneel in a stationary unit's gait slot;
/// movement suppresses it (the chain's locomotion-first order); the flag dropping (the loot
/// release's round-trip) hands the slot back to Stand.
#[test]
fn unit_flag_looting_kneels_stationary_units_only() {
    use benilla_protocol::messages::ObjectFields;

    let mut app = app();
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![clip(0, 1, true), clip(50, 2, false), clip(5, 3, true)],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            crate::net::ObjectStore(ObjectFields::from_pairs(&[(46, 0x400)])),
        ))
        .id();
    let gait = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;

    // Stationary with the flag up: the kneel takes the gait slot.
    app.update();
    assert_eq!(gait(&app), Some(50), "looting kneels");

    // Movement outranks the kneel.
    app.world_mut().entity_mut(unit).insert(MovementState {
        speed: 7.0,
        flags: move_flags::FORWARD,
        ..Default::default()
    });
    app.update();
    assert_eq!(gait(&app), Some(5), "a moving looter runs");

    // Stopped again with the flag down (the release landed): back to Stand.
    app.world_mut().entity_mut(unit).remove::<MovementState>();
    app.world_mut()
        .entity_mut(unit)
        .insert(crate::net::ObjectStore(ObjectFields::from_pairs(&[(
            46, 0,
        )])));
    app.update();
    assert_eq!(gait(&app), Some(0), "released — back to Stand");
}

/// The loot kneel, SELF half (the byte predicate `0x6126b0` splits on
/// IsActivePlayer): the local player's kneel rides the client-local loot-target latch
/// (predicate B's standing answer over the `[player+0x1d28]` latch — [`crate::ui_loot::LootKneel`],
/// decision 1477) — NOT its descriptor flag — so it starts the frame the arm lands
/// (client-predicted, before any server response reaches the descriptor) and ends the frame it
/// drops. *Which* latched objects set that boolean is predicate B's own table, tested where it
/// lives (`ui_loot::tests::predicate_b_decides_which_loot_targets_are_knelt_at`); this test is
/// about the leg reading self and remote from two different places.
#[test]
fn the_self_kneel_rides_the_loot_latch_not_the_flag() {
    use benilla_protocol::messages::ObjectFields;

    let mut app = app();
    app.init_resource::<crate::ui_loot::LootKneel>();
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![clip(0, 1, true), clip(50, 2, false), clip(5, 3, true)],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    // A SELF unit whose descriptor carries UNIT_FLAG_LOOTING but whose latch is empty: no kneel —
    // the flag is the REMOTE trigger only.
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            crate::net::SelfPlayer,
            crate::net::ObjectStore(ObjectFields::from_pairs(&[(46, 0x400)])),
            MovementState::default(),
        ))
        .id();
    let gait = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;

    app.update();
    assert_eq!(
        gait(&app),
        Some(0),
        "the self unit ignores its own descriptor flag"
    );

    // The arm lands on a kneelable target: the kneel is client-predicted the same frame cycle.
    app.world_mut()
        .resource_mut::<crate::ui_loot::LootKneel>()
        .0 = true;
    app.update();
    assert_eq!(gait(&app), Some(50), "the armed latch kneels the self unit");

    // The release/refusal drops the latch: straight back to Stand, no wire round-trip needed.
    app.world_mut()
        .resource_mut::<crate::ui_loot::LootKneel>()
        .0 = false;
    app.update();
    assert_eq!(
        gait(&app),
        Some(0),
        "the dropped latch stands the self unit"
    );
}

/// **B114's second half, end to end**: the prowl pose off the descriptor, through the real driver.
/// The CREEP vis flag (`UNIT_FIELD_BYTES_1` byte 3 bit 1 — field 138, `0x0200_0000`) is the whole
/// gate, and it is read from the unit's own descriptor for the SELF unit too (unlike the loot kneel
/// above, which splits self/remote): there is no client-side prediction of stealth, so the crouch
/// arrives with the server's aura. Stand ⇄ StealthStand and Run ⇄ StealthWalk both flip on the bit
/// alone, with no other state changing.
#[test]
fn the_creep_vis_flag_prowls_the_body() {
    use benilla_protocol::messages::ObjectFields;

    const CREEP: u32 = 0x0200_0000;
    let mut app = app();
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(5, 2, true),   // Run
            clip(119, 3, true), // StealthWalk
            clip(120, 4, true), // StealthStand
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            crate::net::SelfPlayer,
            crate::net::ObjectStore(ObjectFields::from_pairs(&[(138, 0)])),
            MovementState::default(),
        ))
        .id();
    let gait = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;
    let set_flag = |app: &mut App, v: u32| {
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::net::ObjectStore(ObjectFields::from_pairs(&[(
                138, v,
            )])));
    };

    app.update();
    assert_eq!(gait(&app), Some(0), "unstealthed idle stands");

    // The stealth aura landed: the same standing unit drops into the crouch.
    set_flag(&mut app, CREEP);
    app.update();
    assert_eq!(gait(&app), Some(120), "the CREEP bit crouches the idle");

    // Moving while stealthed creeps — at a speed that would otherwise be a flat-out Run.
    app.world_mut().entity_mut(unit).insert(MovementState {
        speed: 7.0,
        flags: move_flags::FORWARD,
        ..Default::default()
    });
    app.update();
    assert_eq!(gait(&app), Some(119), "the prowl outranks the speed tail");

    // Stealth broke mid-run: straight back to the ordinary gait, nothing else touched.
    set_flag(&mut app, 0);
    app.update();
    assert_eq!(gait(&app), Some(5), "broken stealth runs again");
}

/// The looping-variation ADVANCE (the
/// watchdog `0x719370`): a relaxed looping base arm installs a replay window (here `(1,1)` → one
/// pass exactly); each completed window re-arms the id through the weighted, MEMORYLESS variation
/// walk. Over a dozen windows both authored Stand variations must take the main slot — the
/// gryphon's flap/glide alternation and the multi-part /dance in miniature. (The pre-0516 driver
/// armed once and wrapped forever: one variation on screen, the other never.)
#[test]
fn a_looping_arm_advances_through_its_variations_at_window_end() {
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    let mut app = app();
    const DUR: f32 = 0.1;
    let clip_handles: Vec<_> = (0..2)
        .map(|_| {
            let mut c = AnimationClip::default();
            c.set_duration(DUR);
            app.world_mut()
                .resource_mut::<Assets<AnimationClip>>()
                .add(c)
        })
        .collect();
    let (graph, nodes) = AnimationGraph::from_clips(clip_handles);
    let graph_handle = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);
    // Two Stand variations, equal weight, replay (1,1): every window is exactly one pass.
    let variation = |node| {
        let mut c = clip(0, 0, true);
        c.node = node;
        c.duration = DUR;
        c.blend_time = 0.0;
        c.frequency = 0x4000;
        c.replay = (1, 1);
        c
    };
    let anims = ModelAnimations {
        graph: graph_handle.clone(),
        clips: vec![variation(nodes[0]), variation(nodes[1])],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            anims,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
        ))
        .id();

    let mut seen = std::collections::HashSet::new();
    for _ in 0..60 {
        advance(&mut app, 25);
        let tr = app
            .world()
            .entity(unit)
            .get::<AnimationTransitions>()
            .unwrap();
        if let Some(n) = tr.get_main_animation() {
            seen.insert(n);
        }
    }
    assert!(
        seen.contains(&nodes[0]) && seen.contains(&nodes[1]),
        "over ~15 one-pass windows the memoryless weighted walk must visit BOTH variations \
         (saw {seen:?}) — an arm-once-wrap-forever driver never leaves the first"
    );
}

/// A jumper's model for the decision 0864 suite: the airborne bracket (JumpStart/Jump hang/
/// JumpEnd/Fall), the gaits, a spell-kit cast anim (SpellCastOmni 54, with a masked variant),
/// and the combat pair (Special1H 57 / AttackUnarmed 16) for the mid-air fast-path tenant.
fn jumper_model() -> ModelAnimations {
    let mut cast = clip(54, 7, false); // SpellCastOmni — the kit release anim
    cast.upper_node = Some(AnimationNodeIndex::new(8));
    ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(5, 2, true),   // Run
            clip(37, 3, false), // JumpStart
            clip(38, 4, true),  // Jump hang
            clip(39, 5, false), // JumpEnd
            clip(40, 6, true),  // Fall
            cast,
            clip(57, 9, false),  // Special1H — a spell kit's combat one-shot
            clip(16, 10, false), // AttackUnarmed — the bare-hands swing
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    }
}

fn jumper(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            jumper_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            MovementState::default(),
            holding_a_non_weapon(),
        ))
        .id()
}

/// **The director's hillside jump**: a run across broken ground micro-detaches for
/// a frame, and the jump pressed in that window lands and relaunches inside one frame — FALLING
/// never drops, so the old FALLING-edge sample never re-ran and the arc kept the detachment's
/// `jump_arc = false`. The body sailed upward playing the run gait.
#[test]
fn a_jump_out_of_a_micro_detachment_still_enters_the_jump_bracket() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update(); // settle: Stand
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    // One frame of detachment while running: FALLING with a *downward* speed is a step-off arc —
    // no bracket, the gait freezes, which is correct on its own.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING | move_flags::FORWARD,
        speed: 7.0,
        vertical_speed: -0.65,
        ..Default::default()
    });
    app.update();
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Gait,
        "a downward launch is a step-off: the gait holds"
    );
    // The jump fires out of that frame: the body lands and relaunches within the frame, so FALLING
    // is set on this frame too and the bit never toggled. The launch is still a launch.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING | move_flags::FORWARD,
        speed: 7.0,
        vertical_speed: 7.96,
        ..Default::default()
    });
    app.update();
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Entering(super::super::select::Special::Jump),
        "the jump bracket enters even though FALLING never dropped"
    );
}

/// The control on the launch rule: a step-off fall must stay a step-off fall for its whole arc.
/// The gait freeze (`0x5fd8e8`) is the reference's behaviour and the new edge
/// must not reach into it — only a rise *past* the threshold is a launch, and a fall only ever
/// accelerates downward.
#[test]
fn a_deepening_step_off_fall_never_becomes_a_jump() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update();
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    for vz in [-0.65_f32, -3.0, -7.4, -12.0] {
        app.world_mut().entity_mut(unit).insert(MovementState {
            flags: move_flags::FALLING | move_flags::FORWARD,
            speed: 7.0,
            vertical_speed: vz,
            ..Default::default()
        });
        app.update();
        assert_eq!(
            mode(&app),
            super::super::select::Mode::Gait,
            "a step-off fall keeps its frozen gait at vz={vz}"
        );
    }
}

/// The ref's jump-in-place cast (the report this pins): a cast id is CLASS_A
/// but NOT COMBAT, so the airborne route test doesn't mask it — with no move bits it routes
/// FULL-BODY and REPLACES the jump hang on bone 0 (the client's one-slot last-writer-wins; the
/// old machine dropped it, which is why the cast only showed on *walking* jumps). The clip then
/// rides the airborne-freeze: the continuing Jump level must NOT re-preempt it — the client
/// issues no plays mid-arc, so the clip plays out (and a finished one clamps and holds) until
/// an edge.
#[test]
fn a_jump_in_place_cast_replaces_the_hang_and_survives_the_arc() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update(); // settle: Stand
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: 7.9, // an upward launch: a jump arc, in place
        ..Default::default()
    });
    app.update();
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Entering(super::super::select::Special::Jump),
        "airborne: the jump bracket enters"
    );
    // The instant AoE releases mid-air: the kit anim arrives.
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 1,
    });
    app.update();
    fn drv(app: &App, unit: Entity) -> &AnimDriver {
        app.world().entity(unit).get::<AnimDriver>().unwrap()
    }
    assert_eq!(
        drv(&app, unit).mode,
        super::super::select::Mode::Swing {
            id: 54,
            under: Some(super::super::select::Special::Jump),
        },
        "the cast replaces the hang full-body — never dropped, never masked"
    );
    assert!(
        drv(&app, unit).overlay.is_none(),
        "no move bits, non-combat id: not the overlay route"
    );
    // The arc continues: the Jump *level* must not preempt the clip back to the bracket.
    app.update();
    app.update();
    assert!(
        matches!(
            drv(&app, unit).mode,
            super::super::select::Mode::Swing { id: 54, .. }
        ),
        "the airborne-freeze holds the one-shot through the arc"
    );
}

/// Touchdown while a mid-air one-shot holds bone 0: the Special edge (`Some → None`) routes
/// through `leave_special` — the `0x602c60` land dispatcher's pick replaces the clip like any
/// plain play (a stationary landing picks JumpEnd 39).
#[test]
fn landing_mid_cast_plays_the_land_pick() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update();
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: 7.9,
        ..Default::default()
    });
    app.update();
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 1,
    });
    app.update();
    app.world_mut()
        .entity_mut(unit)
        .insert(MovementState::default()); // touchdown, stationary
    app.update();
    let mode = app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    assert_eq!(
        mode,
        super::super::select::Mode::Land { id: 39, flags: 0 },
        "the land pick cuts the held cast at touchdown"
    );
}

/// The FALLINGFAR latch mid-one-shot is an edge (`Jump → Fall`): the client plays Fall(40)
/// ONCE, on the substep it latches (`0x61a820` — the 0864-era per-tick re-assert was
/// refuted), replacing the clip. A fresh cast armed AFTER the latch then
/// holds bone 0 like any other airborne one-shot, until the landing pick cuts it.
#[test]
fn a_cast_over_the_fall_loop_holds_until_landing() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update();
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: 7.9,
        ..Default::default()
    });
    app.update();
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 1,
    });
    app.update(); // Swing { 54, under: Jump }
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING | move_flags::FALLING_FAR,
        vertical_speed: -5.0,
        ..Default::default()
    });
    app.update();
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Looping(super::super::select::Special::Fall),
        "the latch's Fall play replaces the held cast"
    );
    // A second cast while falling far arms (last-writer-wins) …
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 2,
    });
    app.update();
    assert!(
        matches!(
            mode(&app),
            super::super::select::Mode::Swing {
                id: 54,
                under: Some(super::super::select::Special::Fall),
                ..
            }
        ),
        "the cast arms over the Fall loop"
    );
    // … and HOLDS through the rest of the fall: no per-tick re-assert exists, and the
    // Fall level is not an edge.
    app.update();
    app.update();
    assert!(
        matches!(
            mode(&app),
            super::super::select::Mode::Swing {
                id: 54,
                under: Some(super::super::select::Special::Fall),
                ..
            }
        ),
        "the cast holds bone 0 through the fall — Fall plays only at its latch edge"
    );
    // Touchdown cuts it with the land pick, as every airborne one-shot.
    app.world_mut()
        .entity_mut(unit)
        .insert(MovementState::default());
    app.update();
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Land { id: 39, flags: 0 },
        "the landing pick replaces the held cast"
    );
}

/// The walking jump keeps the masked route (the already-working half, now pinned): the
/// takeoff-frozen FORWARD bit routes the cast to the SpineLow overlay — the torso casts, the
/// legs keep the arc, and the base machine is untouched.
#[test]
fn a_moving_jump_cast_masks_onto_the_overlay() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update();
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD | move_flags::FALLING,
        vertical_speed: 7.9,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 1,
    });
    app.update();
    let drv = app.world().entity(unit).get::<AnimDriver>().unwrap();
    assert!(
        drv.overlay.is_some_and(|ov| ov.id == 54),
        "frozen-in move bits: the cast masks onto the overlay"
    );
    assert_eq!(
        drv.mode,
        super::super::select::Mode::Entering(super::super::select::Special::Jump),
        "the base machine keeps the bracket untouched"
    );
}

/// **The transplant** (the director's "jump right after a cast should only play
/// the lower body animation and finish the upper body one"). A standing cast routes full-body to
/// bone 0; the jump that follows is a LOCOMOTION request, so the client does **not** overwrite it
/// — `0x5fe919` copies the bone-0 descriptor (id, rate, and its **live play position**) onto the
/// key-bone and hands bone 0 the jump. The legs jump; the arms finish the cast.
#[test]
fn a_jump_over_a_live_cast_transplants_it_to_the_torso() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update(); // settle: Stand
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 1,
    });
    app.update();
    {
        let drv = app.world().entity(unit).get::<AnimDriver>().unwrap();
        assert!(
            matches!(drv.mode, super::super::select::Mode::Swing { id: 54, .. }),
            "standing: the cast takes the FULL BODY on bone 0"
        );
        assert!(drv.overlay.is_none(), "nothing on the key-bone yet");
    }
    // Jump in place, mid-cast.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: 7.9,
        ..Default::default()
    });
    app.update();
    let drv = app.world().entity(unit).get::<AnimDriver>().unwrap();
    assert_eq!(
        drv.overlay.map(|ov| (ov.id, ov.node)),
        Some((54, AnimationNodeIndex::new(8))),
        "the cast transplants onto the SpineLow overlay instead of being replaced"
    );
    assert_eq!(
        drv.mode,
        super::super::select::Mode::Entering(super::super::select::Special::Jump),
        "…and the legs get the jump"
    );
    assert!(
        drv.overlay_fade.is_none(),
        "a transplant carries blendFlag = 0: it resumes mid-clip, it does not cross-fade"
    );
}

/// A **Special is a bone-0 play, so it cannot cut the torso** (the jump-running
/// half of the director's report). A moving caster's hold rides the key-bone; taking off routes
/// JumpStart to bone 0 (`0x5fe912`: with the key-bone armed the locomotion request goes straight
/// there) and leaves the hold running. The old `special.is_none()` filter dropped it on takeoff.
#[test]
fn a_jump_does_not_cut_the_moving_cast_hold() {
    let mut app = app();
    let unit = app
        .world_mut()
        .spawn((
            caster_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            MovementState {
                flags: move_flags::FORWARD,
                speed: 7.0,
                ..Default::default()
            },
            CastHold {
                anim_id: 51,
                spell_id: 1,
                ranged: false,
            },
        ))
        .id();
    app.update();
    assert!(
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .overlay
            .is_some_and(|ov| ov.id == 51 && ov.looping),
        "a moving caster holds on the torso"
    );
    // Take off, still running, still casting.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD | move_flags::FALLING,
        vertical_speed: 7.9,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    assert!(
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .overlay
            .is_some_and(|ov| ov.id == 51 && ov.looping),
        "the jump takes bone 0 — the hold keeps the torso"
    );
}

/// **The fade-to-rest** ("the end of the cast animation is cut off and it
/// instantly snaps back"). A finished key-bone one-shot is never stopped: the client's completion
/// event disarms the bone through op4 `param_3 = -1`, which holds the clip's final frame in the
/// secondary slot and cross-fades it back onto the base over a fixed 150 ms. Real clip assets, so
/// Bevy actually completes the overlay.
#[test]
fn a_finished_masked_cast_fades_out_instead_of_snapping() {
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    let mut app = app();
    const CAST: f32 = 0.3;
    let run_handle = {
        let mut c = AnimationClip::default();
        c.set_duration(1.0);
        app.world_mut()
            .resource_mut::<Assets<AnimationClip>>()
            .add(c)
    };
    let cast_handle = {
        let mut c = AnimationClip::default();
        c.set_duration(CAST);
        app.world_mut()
            .resource_mut::<Assets<AnimationClip>>()
            .add(c)
    };
    // Three nodes: the run base, the cast's full-body node, and the cast's masked twin.
    let (graph, nodes) = AnimationGraph::from_clips([run_handle, cast_handle.clone(), cast_handle]);
    let graph_handle = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);
    let mut run = clip(5, 0, true);
    run.node = nodes[0];
    let mut cast = clip(54, 0, false);
    cast.node = nodes[1];
    cast.duration = CAST;
    cast.blend_time = 0.05;
    cast.upper_node = Some(nodes[2]);
    let unit = app
        .world_mut()
        .spawn((
            ModelAnimations {
                graph: graph_handle.clone(),
                clips: vec![run, cast],
                hand_close: [None, None],
                playable_animation_lookup: Vec::new(),
                animation_lookup: Vec::new(),
                global_bones: Vec::new(),
                first_seq: None,
                pose: Default::default(),
            },
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
            MovementState {
                flags: move_flags::FORWARD,
                speed: 7.0,
                ..Default::default()
            },
        ))
        .id();
    app.update(); // settle: Run
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 1,
    });
    app.update();
    {
        let e = app.world().entity(unit);
        let drv = e.get::<AnimDriver>().unwrap();
        assert!(
            drv.overlay.is_some_and(|ov| ov.node == nodes[2]),
            "moving: the cast masks onto the torso"
        );
        assert!(
            drv.overlay_fade.is_some_and(|f| f.out.is_none()),
            "the arm is blended: it rises over its own blendTime, from the base pose"
        );
        let w = e.get::<AnimationPlayer>().unwrap().animation(nodes[2]);
        assert!(
            w.is_some_and(|a| a.weight() < super::ONESHOT_OVERLAY_WEIGHT),
            "…so it starts below full weight, not snapped on"
        );
    }
    // Step in small frames until the clip completes (a big frame would run the whole 150 ms fade
    // out in one go — right behaviour, useless assertion).
    for _ in 0..60 {
        advance(&mut app, 20);
        if app
            .world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .overlay
            .is_none()
        {
            break;
        }
    }
    {
        let e = app.world().entity(unit);
        let drv = e.get::<AnimDriver>().unwrap();
        assert!(drv.overlay.is_none(), "the cast finished");
        assert_eq!(
            drv.overlay_fade.and_then(|f| f.out),
            Some(nodes[2]),
            "…and retired into the fade slot rather than being dropped"
        );
        let active = e
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(nodes[2])
            .expect("the finished clip is still driving the torso, holding its last frame");
        assert_eq!(
            active.speed(),
            0.0,
            "held on the final frame, not replaying"
        );
        assert!(active.weight() > 0.0, "still blended in as λ decays");
    }
    // Past the 150 ms window: the slot self-releases, exactly like the kernel's `+0xd0 = -1`.
    advance(&mut app, 200);
    let e = app.world().entity(unit);
    assert!(
        e.get::<AnimDriver>().unwrap().overlay_fade.is_none(),
        "the fade window expired"
    );
    assert!(
        e.get::<AnimationPlayer>()
            .unwrap()
            .animation(nodes[2])
            .is_none(),
        "…and the node is released"
    );
}

/// The airborne-freeze in the GAIT slot (the step-off arc): live pins — here
/// the stationary cast hold — cannot swap the clip mid-air; the takeoff gait keeps rolling and
/// the pin applies at touchdown.
#[test]
fn the_step_off_arc_freezes_the_gait_against_live_pins() {
    let mut app = app();
    let unit = app
        .world_mut()
        .spawn((
            caster_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            MovementState::default(),
        ))
        .id();
    app.update(); // settle: Stand
    let gait = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;
    assert_eq!(gait(&app), Some(0));
    // Step off a ledge (downward launch: no jump arc, no Special; vz ≠ 0 — the freeze gate
    // `0x5fd8e8`, `FALLING && (FALLINGFAR || vz ≠ 0)`) …
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: -3.0,
        ..Default::default()
    });
    // … and the precast lands mid-fall: the stationary pin must NOT re-pick mid-air.
    app.world_mut().entity_mut(unit).insert(CastHold {
        ranged: false,
        anim_id: 51,
        spell_id: 20793,
    });
    app.update();
    app.update();
    assert_eq!(
        gait(&app),
        Some(0),
        "the selector never re-picks mid-air — the takeoff gait holds"
    );
    // Touchdown: the freeze lifts and the pin applies immediately.
    app.world_mut()
        .entity_mut(unit)
        .insert(MovementState::default());
    app.update();
    assert_eq!(gait(&app), Some(51), "the pin lands with the unit");
}

/// A mid-air fast-path park survives the arc's LEVEL (decision 0864's edge-clear): the old
/// per-frame kill cleared the deferred cache on every airborne frame; the client clears only
/// at plays (state EDGES) — and the landing's pick, a play, still kills it.
#[test]
fn a_midair_deferred_park_survives_the_level_and_dies_at_the_landing_play() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update();
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: 7.9,
        ..Default::default()
    });
    app.update(); // Entering(Jump)
                  // A kit combat one-shot replaces the bracket (57 is forced-full-body), then a swing lands
                  // the same wire batch: combat-over-combat fast-paths — the swing parks.
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 57,
        seq: 1,
    });
    app.world_mut().write_message(SwingMessage {
        attacker: unit,
        victim: None,
        hit_info: 0x2,
        victim_state: 1,
        damage: 21,
        displayed: true,
        seq: 2,
    });
    app.update();
    fn drv(app: &App, unit: Entity) -> &AnimDriver {
        app.world().entity(unit).get::<AnimDriver>().unwrap()
    }
    assert_eq!(
        drv(&app, unit).deferred,
        Some(16),
        "the swing parks behind the kit clip"
    );
    // The arc's level must not kill the park (the old per-frame clear did).
    app.update();
    app.update();
    assert_eq!(
        drv(&app, unit).deferred,
        Some(16),
        "no play mid-arc — the park survives"
    );
    // Touchdown: the land pick is a play — the park dies with it (`0x5fe48e`).
    app.world_mut()
        .entity_mut(unit)
        .insert(MovementState::default());
    app.update();
    assert_eq!(
        drv(&app, unit).deferred,
        None,
        "the landing play clears the cache"
    );
}

/// The deferred cache's consuming read sits DOWNSTREAM of the airborne-freeze (`0x5fd392`
/// inside the `0x5fd360` recompute arm): a park made mid-arc is
/// never consumed mid-air, even with the body free — it waits, and the landing edge's play
/// clears it.
#[test]
fn a_midair_park_is_not_consumed_before_landing() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update();
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: 7.9,
        ..Default::default()
    });
    app.update(); // Entering(Jump), body otherwise free
    app.world_mut()
        .entity_mut(unit)
        .get_mut::<AnimDriver>()
        .unwrap()
        .deferred = Some(16);
    app.update();
    app.update();
    fn drv(app: &App, unit: Entity) -> &AnimDriver {
        app.world().entity(unit).get::<AnimDriver>().unwrap()
    }
    assert_eq!(
        drv(&app, unit).deferred,
        Some(16),
        "the freeze blocks the consuming read — the park waits mid-air"
    );
    assert!(
        drv(&app, unit).overlay.is_none(),
        "the parked swing never played mid-air"
    );
    // Touchdown: the landing play clears the cache (`0x5fe48e`) — the park dies unplayed.
    app.world_mut()
        .entity_mut(unit)
        .insert(MovementState::default());
    app.update();
    assert_eq!(
        drv(&app, unit).deferred,
        None,
        "the landing play clears the cache"
    );
}

/// **The ranged→melee handoff at every landed swing** (`0x625829` in
/// `0x6255b0`) — the director's report: a bow drawn by a shot that never fired, then a melee
/// attack, and the swings keep coming out of the bow. The reconcile provably cannot fix it, and
/// this pins both halves: the CONTROL (a sword swing while ranged-drawn leaves the stance at 2 —
/// the client's melee force is gated `CUR != 2`, `0x5fe0f9`/`0x5fe13b`, so the ranged stance is
/// stable under any number of swings), and the FIX (the packet arm's own snap moves it, and the
/// reconcile then holds it there).
#[test]
fn a_landed_swing_snaps_its_attacker_out_of_the_ranged_stance() {
    let mut app = app();
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![clip(0, 1, true), clip(17, 2, false)], // Stand + Attack1H
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    // A warrior wearing a 1H sword and a bow: the exact loadout of the report.
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            crate::net::SelfPlayer,
            Wielded {
                main: Some((2, 0x7)), // 1H sword -> Attack1H (17)
                off: None,
                ranged: Some((2, 0x2)), // bow
                main_sheath: 3,
                off_sheath: 0,
                ..Default::default()
            },
        ))
        .id();
    let sheath = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .sheath_state()
    };
    let swing = |app: &mut App| {
        app.world_mut().write_message(SwingMessage {
            attacker: unit,
            victim: None,
            hit_info: 0,
            victim_state: 1,
            damage: 7,
            displayed: true,
            seq: 0,
        });
        app.update();
    };

    // The shot's draw (`SetSheatheState(2, SNAP)` — the cast-send arm).
    app.world_mut().write_message(SheathRequest {
        entity: unit,
        state: 2,
        ceremony: false,
    });
    app.update();
    assert_eq!(sheath(&app), Some(2), "the shot draws the bow");

    // CONTROL: swings alone. Attack1H's WeaponFlags `&0x20` is a force-DRAW-MELEE the client only
    // consults on the `CUR != 2` path, so nothing in the reconcile can leave the ranged stance.
    swing(&mut app);
    swing(&mut app);
    assert_eq!(
        sheath(&app),
        Some(2),
        "the reconcile alone never leaves the ranged stance — this is the bug's shape"
    );

    // The packet arm's snap (what `creature_anim::net::attacker_state` now writes beside the
    // swing): the sword comes out on the first landed blow.
    app.world_mut().write_message(SheathRequest {
        entity: unit,
        state: 1,
        ceremony: false,
    });
    swing(&mut app);
    assert_eq!(sheath(&app), Some(1), "the landed swing draws melee");

    // …and holds: every later swing re-requests 1, which the setter refuses as idempotent, and
    // Attack1H's `&0x20` re-asserts melee on the `CUR != 2` path.
    swing(&mut app);
    assert_eq!(sheath(&app), Some(1), "melee holds across the volley");
}

/// **The ceremony's two movements, end to end** — the director's report: pressing
/// Z with both hands full puts the weapons away, lets the arms come back to neutral, and only
/// *then* reaches over the shoulder for the bow. Phase 1 is the setter's own play (`0x611b60`);
/// phase 2 is the on-anim-finish drawer (`0x5fc920` @ `0x5fca8c`/`0x5fcaa1`), which is the half
/// benilla never had — before it, the ceremony ended when the stow clips did and the bow simply
/// appeared. Real clip assets, so Bevy's `advance_animations` actually completes them.
#[test]
fn a_melee_to_ranged_toggle_stows_both_hands_before_it_reaches_for_the_bow() {
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    let mut app = app();
    const DUR: f32 = 0.1;
    let handles: Vec<_> = (0..5)
        .map(|_| {
            let mut c = AnimationClip::default();
            c.set_duration(DUR);
            app.world_mut()
                .resource_mut::<Assets<AnimationClip>>()
                .add(c)
        })
        .collect();
    let (graph, nodes) = AnimationGraph::from_clips(handles);
    let graph_handle = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);
    // The two stow/draw families, each with its per-arm masked pair. No `$SHL`/`$SHR` events, so
    // each arm's weapon moves at the authored-event fallback: halfway.
    let family = |id: u16, right: usize, left: usize| {
        let mut c = clip(id, 0, false);
        c.node = nodes[right];
        c.duration = DUR;
        c.blend_time = 0.0;
        c.arm_nodes = Some((nodes[right], nodes[left]));
        c
    };
    let mut stand = clip(0, 0, true);
    stand.node = nodes[0];
    stand.duration = DUR;
    let anims = ModelAnimations {
        graph: graph_handle.clone(),
        clips: vec![stand, family(90, 1, 2), family(89, 3, 4)],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    // Sword-and-board plus a bow — the director's warrior. Hip sword (3 ⇒ HipSheath 90), back
    // shield (4 ⇒ Sheath 89), back bow (1 ⇒ 89, and INVTYPE_RANGED ⇒ the LEFT arm).
    let unit = app
        .world_mut()
        .spawn((
            anims,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
            // The Z toggle is the local player's alone; a remote unit's committed state is pulled
            // back to the server byte by the reconcile's rule 5 before any ceremony could run.
            crate::net::SelfPlayer,
            Wielded {
                main: Some((2, 0x7)),
                off: Some((4, 6)),
                ranged: Some((2, 0x2)),
                main_sheath: 3,
                off_sheath: 4,
                ranged_sheath: 1,
                ranged_inv: 0x0f,
                materials: [1, 6, 2],
                disarmed: false,
            },
        ))
        .id();
    let visual = |app: &App| {
        app.world()
            .entity(unit)
            .get::<crate::creature_anim::VisualSheath>()
            .map(|v| v.0)
    };

    // Get to melee-drawn without a ceremony (a snap, as every reactive trigger does), then press Z.
    for (state, ceremony) in [(1u8, false), (2, true)] {
        app.world_mut().write_message(SheathRequest {
            entity: unit,
            state,
            ceremony,
        });
        app.update();
    }
    assert_eq!(
        visual(&app),
        Some([1, 1]),
        "phase 1: both hands still hold their weapons while the stow clips play"
    );

    let mut seen = vec![[1u8, 1]];
    let mut settled = false;
    for _ in 0..60 {
        advance(&mut app, 20);
        match visual(&app) {
            Some(v) if seen.last() != Some(&v) => seen.push(v),
            None => {
                settled = true;
                break;
            }
            _ => {}
        }
    }

    // The smoking gun: a state where the RIGHT arm has settled into the ranged stance (sword on
    // the back, nothing left to do) while the LEFT is still empty-handed — the bow on its way but
    // not yet arrived. That can only exist if a second clip started after the stows finished.
    assert!(
        seen.contains(&[2, 0]),
        "phase 2 never ran: the bow must be drawn by a SECOND clip, after both stows finished \
         (saw {seen:?})"
    );
    let stow = seen.iter().position(|v| *v == [1, 1]).unwrap();
    let reach = seen.iter().position(|v| *v == [2, 0]).unwrap();
    assert!(
        reach > stow,
        "the reach must follow the stow, not blend with it (saw {seen:?})"
    );
    assert!(
        settled,
        "the ceremony must end with the pin dropped, leaving the committed state (saw {seen:?})"
    );
}

/// **Ice Block, at the mechanism.** The stun's root wipes the direction bits in the
/// SAME frame the cast one-shot arrives, so the one-shot's own arm-time flags already read ROOT and
/// an arm-time comparison sees no movement edge — ever. The base's flags still hold the run it was
/// armed for, so the edge is there, the re-arm resolves to **Stand(0)** (not locomotion → no
/// transplant), and Stand overwrites the cast on bone 0: the character is fully neutral, with
/// nothing on the torso, by the time the freeze catches it. Before this the cast held bone 0 for the
/// whole block, or rode up to the torso and froze an arm out.
#[test]
fn a_root_landing_with_the_cast_returns_the_body_to_neutral() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    let gait = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    let overlay = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .overlay
            .map(|o| o.id)
    };
    assert_eq!(gait(&app), Some(5), "running");

    // One frame: the root lands (direction bits wiped) AND the cast one-shot arrives.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::ROOT,
        ..Default::default()
    });
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 1,
    });
    app.update();
    // The cast takes bone 0 and the base re-arm displaces it inside the same frame — the reference's
    // one-slot last-writer-wins, which is exactly what the base's flags (still FORWARD) unlock.
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Gait,
        "the base request wins bone 0 back"
    );
    assert!(
        overlay(&app).is_none(),
        "no torso transplant: Stand(0) is not a locomotion request"
    );
    app.update();
    assert_eq!(gait(&app), Some(0), "fully neutral standing");
    assert!(overlay(&app).is_none(), "and nothing left on the torso");
}

/// The control that keeps 0878 honest: the transplant still fires when the re-arm really *is*
/// locomotion. A cast fired standing, then the player runs — the legs take Run and the cast keeps
/// going on the torso, which is the whole of R-B.
#[test]
fn a_run_starting_under_a_cast_still_transplants_it_up() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update(); // settle: Stand
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 54,
        seq: 1,
    });
    app.update();
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    let overlay = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .overlay
            .map(|o| o.id)
    };
    assert!(matches!(
        mode(&app),
        super::super::select::Mode::Swing { id: 54, .. }
    ));

    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    assert_eq!(
        overlay(&app),
        Some(54),
        "a locomotion re-arm moves the cast to the torso instead of cutting it"
    );
}

/// A walker's model: Stand plus a Walk(4) authored at the 2.5 yd/s design speed every 1.12.1
/// creature and character rig shares (byte-read from `ogremage.m2` and `humanmale.m2` with
/// `benilla-extract m2seq`).
fn walker_model() -> ModelAnimations {
    let mut walk = clip(4, 2, true);
    walk.move_speed = 2.5;
    ModelAnimations {
        graph: Handle::default(),
        clips: vec![clip(0, 1, true), walk],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    }
}

/// A **mount's** model, as the real assets are shaped: Horse.m2 authors no
/// JumpLandRun 187 at all, and its baked PlayableAnimationLookup answers a 187 request with
/// **Run(5)** — `playable[187] = 5`, the same row Tiger.m2 (the druid travel form) and Cat.m2
/// carry. The Run clip's authored design speed is the horse's real 9.028 yd/s.
fn mount_model() -> ModelAnimations {
    let mut run = clip(5, 2, true);
    run.move_speed = 9.028; // Horse.m2 sequence 16's ModelAnimation::move_speed
    let mut table = vec![
        benilla_formats::PlayableAnim {
            resolved_id: 0,
            dir_flags: 0,
        };
        203
    ];
    for id in [0u16, 5, 37, 38, 39] {
        table[id as usize].resolved_id = id;
    }
    // A mount authors neither Sprint 143 (what the selector picks at mount speed) nor
    // JumpLandRun 187 (what the landing picks): its baked table answers BOTH with the gallop
    // cycle, so the whole galloping-jump-landing sequence is one clip at three rates.
    table[143].resolved_id = 5;
    table[187].resolved_id = 5;
    ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),
            run,
            clip(37, 3, false), // JumpStart
            clip(38, 4, true),  // Jump hang
            clip(39, 5, false), // JumpEnd
        ],
        hand_close: [None, None],
        playable_animation_lookup: table,
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    }
}

/// The rate the driver actually wrote onto the walk node — the number the fix is about, read back
/// off the live `AnimationPlayer` rather than off the driver's own bookkeeping.
fn walk_rate(app: &App, unit: Entity) -> f32 {
    app.world()
        .entity(unit)
        .get::<AnimationPlayer>()
        .unwrap()
        .animation(AnimationNodeIndex::new(2))
        .expect("the walk node is playing")
        .speed()
}

/// The director's ogre, end to end through the real system: a Gordok Ogre-Mage
/// (`CreatureDisplayInfo.CreatureModelScale` 2.2, so `OBJECT_FIELD_SCALE_X` 2.2) walking at
/// vmangos' `speed_walk` 1.6 × 2.5 = 4.0 yd/s must cycle its legs at 4.0 / (2.5 × 2.2) = 0.73×.
/// Scale-blind it read 1.60× — the "too fast walk" report. The control is the same unit at scale
/// 1.0, which must still read the un-divided 1.60×: the fix may not touch ordinary-size creatures.
#[test]
fn a_scaled_creatures_walk_cycles_slower_than_an_unscaled_ones() {
    let walking = MovementState {
        flags: move_flags::FORWARD,
        speed: 4.0,
        ..Default::default()
    };
    let mut app = app();
    let ogre = app
        .world_mut()
        .spawn((
            walker_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Transform::from_scale(Vec3::splat(2.2)),
            walking,
        ))
        .id();
    let human = app
        .world_mut()
        .spawn((
            walker_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Transform::from_scale(Vec3::splat(1.0)),
            walking,
        ))
        .id();
    app.update();
    assert!(
        (walk_rate(&app, ogre) - 0.727_27).abs() < 1e-3,
        "the 2.2x ogre walks its cycle 2.2x slower, not at the scale-blind 1.60x: got {}",
        walk_rate(&app, ogre)
    );
    assert!(
        (walk_rate(&app, human) - 1.6).abs() < 1e-3,
        "an unscaled unit is untouched by the divisor: got {}",
        walk_rate(&app, human)
    );
}

/// The mounted half of the same law: the client's divisor reads the MOUNT model
/// (`[unit+0xdc] ?: [unit+0xd8]`), whose rendered scale is the rider's `OBJECT_FIELD_SCALE_X`
/// times the mount's own `CreatureDisplayInfo` column (`0x613ef0`). Our mount child carries
/// only that column on its transform, so the driver must compose the host's in — a 1.5× sabre
/// under a 2.0× rider divides by 3.0, not 1.5.
#[test]
fn a_mounts_gait_rate_composes_the_riders_scale_with_the_mounts() {
    let mut app = app();
    let rider = app
        .world_mut()
        .spawn((
            Transform::from_scale(Vec3::splat(2.0)),
            MovementState {
                flags: move_flags::FORWARD,
                speed: 4.0,
                ..Default::default()
            },
        ))
        .id();
    let mount = app
        .world_mut()
        .spawn((
            walker_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Transform::from_scale(Vec3::splat(1.5)),
            crate::entities::mount::MountBody { host: rider },
        ))
        .id();
    app.update();
    // 4.0 / (2.5 · 1.5 · 2.0) = 0.533… — the mount also inherits the rider's movement view, so the
    // speed feeding the divide is the rider's, exactly as the client's per-unit call reads it.
    assert!(
        (walk_rate(&app, mount) - 0.533_33).abs() < 1e-3,
        "the mount divides by rider x mount scale: got {}",
        walk_rate(&app, mount)
    );
}

/// The director's report: "jumping while running forward, mounted or in druid
/// travel form, slows the running animation on landing and it takes ~1-2 s to snap back".
///
/// The landing request is JumpLandRun 187; on every creature model that resolves to **Run(5)**,
/// a rate-scaled locomotion clip — so the land clip IS the gallop cycle and must run at
/// `speed / moveSpeed` like the gait it continues. It used to be armed at the call site's literal
/// `1.0` and nothing rewrote it until `Mode::Land` ended, which is the clip's whole length of
/// visibly-slow legs. The rate write is per-frame and mode-independent now (`sync_base_rate`), so
/// the landing runs at the same rate the airborne gait did.
#[test]
fn a_mounted_landing_runs_at_the_gaits_rate_not_at_one_times() {
    let mut app = app();
    let unit = app
        .world_mut()
        .spawn((
            mount_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Transform::from_scale(Vec3::splat(1.0)),
            MovementState::default(),
        ))
        .id();
    // Galloping forward at a 100% mount's 14 yd/s: Run at 14 / 9.028 ≈ 1.551×.
    let running = MovementState {
        flags: move_flags::FORWARD,
        speed: 14.0,
        ..Default::default()
    };
    app.world_mut().entity_mut(unit).insert(running);
    app.update();
    let rate = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(AnimationNodeIndex::new(2)) // the Run node
            .map(|a| a.speed())
    };
    let expected = 14.0 / 9.028;
    assert!(
        rate(&app).is_some_and(|r| (r - expected).abs() < 1e-4),
        "the gallop scales by speed: {:?} vs {expected}",
        rate(&app)
    );

    // Up: the jump bracket takes the body (JumpStart, then the hang).
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD | move_flags::FALLING,
        speed: 14.0,
        vertical_speed: 7.9,
        ..Default::default()
    });
    app.update();
    assert!(
        matches!(
            drv_of(&app, unit).mode,
            super::super::select::Mode::Entering(super::super::select::Special::Jump)
        ),
        "an upward launch enters the jump bracket"
    );

    // Touchdown, still holding forward: the land pick is 187 → the model plays Run for it.
    app.world_mut().entity_mut(unit).insert(running);
    app.update();
    assert_eq!(
        drv_of(&app, unit).mode,
        super::super::select::Mode::Land {
            id: 187,
            flags: move_flags::FORWARD
        },
        "a forward landing picks JumpLandRun"
    );
    assert!(
        rate(&app).is_some_and(|r| (r - expected).abs() < 1e-4),
        "the landing clip is the gallop cycle and runs at the gallop's rate: {:?} vs {expected}",
        rate(&app)
    );
}

fn drv_of(app: &App, unit: Entity) -> &AnimDriver {
    app.world().entity(unit).get::<AnimDriver>().unwrap()
}

/// **B203 — the mount transition's own bone-0 arm.** The mount summon's cast-stage kit plays
/// `SpellCastOmni` (54) at SPELL_GO, while the caster is still *unmounted*, so it takes the
/// full-body route and `Mode::Swing` owns bone 0 for the clip's whole span. The mount field then
/// lands a beat later — and the gait slot's mounted pin cannot help, because it only picks when
/// the mode is already `Mode::Gait`. The reference has no such wait: `0x607a00`'s tail
/// (`0x607b44`) is an ordinary op4 PRIMARY play of **91 `Mount`** on bone 0 of the body, so the
/// cast clip is displaced the moment the mount arrives — which is why the director sees no cast
/// animation at mount-up, only the poof.
#[test]
fn the_mount_transition_takes_bone_0_back_from_a_full_body_one_shot() {
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_MOUNTDISPLAYID` (index 133).
    const FIELD_MOUNTDISPLAYID: u16 = 133;
    const SPELL_CAST_OMNI: u16 = 54;
    const MOUNT: u16 = 91;

    let mut app = app();
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),                // Stand
            clip(MOUNT, 2, true),            // Mount — the seat pose
            clip(SPELL_CAST_OMNI, 3, false), // the cast release, FULL BODY (no upper_node)
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            crate::net::ObjectStore(ObjectFields::from_pairs(&[(FIELD_MOUNTDISPLAYID, 0)])),
            MovementState::default(),
        ))
        .id();
    let mount_field = |app: &mut App, v: u32| {
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::net::ObjectStore(ObjectFields::from_pairs(&[(
                FIELD_MOUNTDISPLAYID,
                v,
            )])));
    };
    let playing = |app: &App| drv_of(app, unit).active_anim();

    app.update();
    assert_eq!(playing(&app), Some(0), "standing, unmounted");

    // SPELL_GO: the cast-stage kit's release clip, full-body over bone 0.
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: SPELL_CAST_OMNI,
        seq: 1,
    });
    app.update();
    assert_eq!(
        playing(&app),
        Some(SPELL_CAST_OMNI),
        "the release clip takes bone 0 while the caster is still on foot"
    );

    // …and the mount field lands, mid-clip. The transition's own arm reclaims bone 0.
    mount_field(&mut app, 2404);
    app.update();
    assert_eq!(
        playing(&app),
        Some(MOUNT),
        "B203: the mount arm displaces the cast clip — it does not wait for it to finish"
    );

    // The edge is a CHANGE, not a level: a steady mounted frame re-picks nothing new, and the
    // seat pose simply holds (the reference's per-play re-force, rendered by the gait pin).
    app.update();
    assert_eq!(playing(&app), Some(MOUNT));

    // Dismount is the same watcher's other leg (`0x607ce0` arms seq 0 Stand on the same bone).
    mount_field(&mut app, 0);
    app.update();
    assert_eq!(playing(&app), Some(0), "back on its own feet, standing");
}

/// **B204 — the dismount CUTS the saddle pose; the mount-up blends into it**.
/// The two legs of the `UNIT_FIELD_MOUNTDISPLAYID` watcher issue the same seven-argument op4
/// `0x7121a0` call and differ in exactly one literal: `0x607b35 push 0x1` (build, cross-fade) vs
/// `0x607d1c push 0x0` (teardown, no cross-fade). Fading Mount(91) out instead of cutting it is
/// what read as a landing — 91 splays and bends the legs, and easing that into Stand over the
/// clip's blend is a body absorbing an impact.
///
/// The assertion is on the OUTGOING clip's weight, because that is the whole difference: after
/// the build frame the old pose is still weighted in (a blend in progress), after the teardown
/// frame the saddle pose contributes nothing at all.
#[test]
fn the_dismount_cuts_the_saddle_pose_where_the_mount_up_blends_into_it() {
    use benilla_protocol::ObjectFields;

    const FIELD_MOUNTDISPLAYID: u16 = 133;
    const MOUNT: u16 = 91;
    let stand_node = AnimationNodeIndex::new(1);
    let mount_node = AnimationNodeIndex::new(2);

    let mut app = app();
    // A deliberately LONG blend on both clips, so "did it fade or did it cut" cannot come down to
    // how long a headless frame happened to take: at the harness's real-time `dt` the shipped
    // 0.15 s blend is all but finished after a single update, and the difference the test is
    // about would sit inside the noise.
    let mut stand = clip(0, 1, true);
    let mut mount = clip(MOUNT, 2, true);
    stand.blend_time = 2.0;
    mount.blend_time = 2.0;
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![stand, mount],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let unit = app
        .world_mut()
        .spawn((
            model,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            crate::net::ObjectStore(ObjectFields::from_pairs(&[(FIELD_MOUNTDISPLAYID, 0)])),
            MovementState::default(),
        ))
        .id();
    let mount_field = |app: &mut App, v: u32| {
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::net::ObjectStore(ObjectFields::from_pairs(&[(
                FIELD_MOUNTDISPLAYID,
                v,
            )])));
    };
    let weight = |app: &App, node| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(node)
            .map_or(0.0, |a| a.weight())
    };

    app.update();
    assert_eq!(drv_of(&app, unit).active_anim(), Some(0));

    // Build leg — `0x607b35 push 0x1`: Stand is still fading out under the seat pose.
    mount_field(&mut app, 2404);
    app.update();
    assert_eq!(drv_of(&app, unit).active_anim(), Some(MOUNT));
    assert!(
        weight(&app, stand_node) > 0.5,
        "the mount-up cross-fades: the outgoing pose is still weighted in"
    );

    // Teardown leg — `0x607d1c push 0x0`: the saddle pose is gone on the arm's own frame, not
    // eased away over the clip's blend time.
    mount_field(&mut app, 0);
    app.update();
    assert_eq!(drv_of(&app, unit).active_anim(), Some(0));
    assert_eq!(
        weight(&app, mount_node),
        0.0,
        "the dismount CUTS: Mount(91) contributes nothing the frame the field clears"
    );
}

/// A shooter's model: Stand, Run, and the bow Load/Hold pair.
fn archer_model() -> ModelAnimations {
    ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),    // Stand
            clip(5, 2, true),    // Run
            clip(105, 3, false), // LoadBow — the pull
            clip(109, 4, true),  // HoldBow — the drawn hold
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    }
}

/// The bow-and-arrow half of the director's 2026-08-05 report — the screenshot is a warrior
/// **sprinting** with the arrow still nocked and the bowstring still drawn.
///
/// The nock latch has exactly two authored writers: `$BWP` sets it, `$BWR` clears it, and both
/// tags live only in clips a STANDING unit plays (the Load pull and the fire clip; a real
/// character M2's Run authors neither — verified by dumping every playable model's event tracks).
/// So a latch carried into locomotion could never be cleared by anything, and the gait arm's
/// hold-pick re-latch (0409's INTERIM) re-arms it on every volley — the leak was guaranteed.
#[test]
fn a_running_shooter_drops_the_nocked_arrow_and_keeps_its_ammo_cache() {
    let mut app = app();
    let unit = app
        .world_mut()
        .spawn((
            archer_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            crate::net::SelfPlayer,
            Wielded {
                ranged: Some((2, 0x2)), // bow
                ..Default::default()
            },
            crate::creature_anim::NockedAmmo { display_id: 5996 },
            crate::creature_anim::NockLatch,
        ))
        .id();
    // The bow is drawn and the arrow is on the string: the steady state of a standing shooter.
    app.world_mut().write_message(SheathRequest {
        entity: unit,
        state: 2,
        ceremony: false,
    });
    app.update();
    let latched = |app: &App| {
        app.world()
            .entity(unit)
            .get::<crate::creature_anim::NockLatch>()
            .is_some()
    };
    assert!(latched(&app), "standing drawn: the arrow stays nocked");

    // They run.
    app.world_mut().entity_mut(unit).insert(MovementState {
        speed: 7.0,
        flags: move_flags::FORWARD,
        ..Default::default()
    });
    app.update();
    assert!(
        !latched(&app),
        "moving un-nocks: the arrow leaves the hand and the string relaxes"
    );
    assert!(
        app.world()
            .entity(unit)
            .get::<crate::creature_anim::NockedAmmo>()
            .is_some(),
        "…but the ammo DISPLAY cache survives — the next pull re-nocks the same arrow without \
         waiting on a fresh SMSG_SPELL_START"
    );
}

/// The aiming half of the same report ("they keep aiming like they are going to shoot at
/// something"): the drawn Load/Hold idle is entered by the LOCAL auto-repeat bit `0x200` alone
/// (`0x5fd460`'s only claim test). The any-caster weapon-visual hold `0x400` — which every
/// ranged-slot spell's visual sets and nothing ever clears on volley end — must not admit it.
#[test]
fn the_weapon_visual_hold_alone_never_puts_a_shooter_in_the_drawn_idle() {
    let spawn = |app: &mut App, auto_repeat: bool| {
        let mut e = app.world_mut().spawn((
            archer_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            crate::net::SelfPlayer,
            Wielded {
                ranged: Some((2, 0x2)),
                ..Default::default()
            },
            // Set by ANY ranged spell's visual play — one Multi-Shot is enough, and it is still
            // set an hour later.
            crate::creature_anim::RangedHold,
        ));
        if auto_repeat {
            e.insert(crate::creature_anim::AutoRepeatArmed);
        }
        e.id()
    };
    let mut app = app();
    let shot_once = spawn(&mut app, false);
    let shooting = spawn(&mut app, true);
    for unit in [shot_once, shooting] {
        app.world_mut().write_message(SheathRequest {
            entity: unit,
            state: 2,
            ceremony: false,
        });
    }
    app.update();
    app.update();
    let gait = |app: &App, e: Entity| app.world().entity(e).get::<AnimDriver>().unwrap().gait;
    assert_eq!(
        gait(&app, shot_once),
        Some(0),
        "a hunter who fired one Multi-Shot and stopped stands normally — the bow stays drawn \
         (nothing stows on combat end), but they are not aiming it"
    );
    assert_eq!(
        gait(&app, shooting),
        Some(105),
        "…while an actively auto-repeating shooter pulls the bow: the `0x200` entry"
    );
}

/// The mid-volley half of the same report ("when it's on and I'm running it keeps repeating the
/// aim animation weirdly") — **re-derived, and inverted, by decision 1544.**
///
/// 0994 read the reference as: the completion dispatcher `0x5fc3f0` is never reached for a bow
/// id, so a finished AttackBow recomputes nothing and clamps on its tail. That absence proof is
/// wrong — the dispatcher has a SECOND, deferred fire site (`0x719370`
/// enqueues the callback as a plain argument with mode 0; `0x7074b0` invokes it later as
/// `call [esi+4]`, which an instruction-encoding census cannot see); its jump table:
/// 46/49/107 land on slot 22, a bare `RecomputeBaseAnim(-1)`, and a finished Load lands on slot
/// 11/12/15, which arms the Hold **unconditionally**.
///
/// So the mid-volley cycle is **fire → re-pull → hold**, once per shot. That re-pull is the
/// "reload" of bug B307: `$BWP` lives only in the Load clips (verified on five shipped character
/// models), so it is the only thing that can put the arrow back on the string — and holding it
/// out left every shot after the first firing from an empty hand.
///
/// What 0994 got right and this keeps: the director's original complaint was about a *moving*
/// shooter, and locomotion still outranks the drawn idle, so nothing here re-pulls mid-run.
#[test]
fn a_mid_volley_fire_clip_re_pulls_and_the_pull_promotes_to_the_hold() {
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    // Stand-in spans; only the ORDER of completions is load-bearing, not the numbers.
    const PULL: f32 = 0.7;
    const FIRE: f32 = 0.5;

    let mut app = app();
    let asset = |app: &mut App, secs: f32| {
        let mut c = AnimationClip::default();
        c.set_duration(secs);
        app.world_mut()
            .resource_mut::<Assets<AnimationClip>>()
            .add(c)
    };
    let (stand, pull, fire, hold) = (
        asset(&mut app, 1.0),
        asset(&mut app, PULL),
        asset(&mut app, FIRE),
        asset(&mut app, 1.0),
    );
    let (graph, nodes) = AnimationGraph::from_clips([stand, pull, fire, hold]);
    let graph_handle = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);

    // A real shooter's model authors all four (HumanMale: 0, 105, 46, 109 — `benilla-extract
    // … m2seq`), and 109 is the only one of them authored as a LOOP.
    let mut stand_clip = clip(0, 0, true);
    stand_clip.node = nodes[0];
    let mut pull_clip = clip(105, 0, false);
    pull_clip.node = nodes[1];
    pull_clip.duration = PULL;
    let mut fire_clip = clip(46, 0, false);
    fire_clip.node = nodes[2];
    fire_clip.duration = FIRE;
    let mut hold_clip = clip(109, 0, true);
    hold_clip.node = nodes[3];
    hold_clip.duration = 1.0;

    let unit = app
        .world_mut()
        .spawn((
            ModelAnimations {
                graph: graph_handle.clone(),
                clips: vec![stand_clip, pull_clip, fire_clip, hold_clip],
                hand_close: [None, None],
                playable_animation_lookup: Vec::new(),
                animation_lookup: Vec::new(),
                global_bones: Vec::new(),
                first_seq: None,
                pose: Default::default(),
            },
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
            crate::net::SelfPlayer,
            Wielded {
                ranged: Some((2, 0x2)), // bow
                ..Default::default()
            },
            crate::creature_anim::AutoRepeatArmed,
        ))
        .id();
    app.world_mut().write_message(SheathRequest {
        entity: unit,
        state: 2,
        ceremony: false,
    });
    app.update();
    app.update();
    let gait = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;

    assert_eq!(gait(&app), Some(105), "the volley opens with the pull");

    // …which promotes to the HOLD on its own completion — slot 11, unconditional. The extra
    // frame is the schedule, not a fudge: the driver runs in `Update` and Bevy advances the
    // clips in `PostUpdate`, so a completion is visible to the machine on the frame AFTER the
    // one that finished it.
    advance(&mut app, 1000);
    app.update();
    assert_eq!(
        gait(&app),
        Some(109),
        "a finished LoadBow yields HoldBow — the drawn pose the shooter sits in between shots"
    );

    // Shot 1: the fire clip takes the body as a one-shot, through the real message lane.
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 46,
        seq: 1,
    });
    app.update();
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Swing {
            id: 46,
            under: None,
        },
        "AttackBow takes bone 0"
    );

    // Its completion recomputes — and for an armed shooter the base re-picks the pull. THIS is
    // the reload the report was missing.
    advance(&mut app, 1000);
    app.update();
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Gait,
        "the fire clip's completion recomputes the base (slot 22's bare RecomputeBaseAnim(-1))"
    );
    // The recompute clears the gait and re-picks it on the following frame (`drv.gait = None`).
    app.update();
    assert_eq!(
        gait(&app),
        Some(105),
        "…and the shooter RE-PULLS: the per-shot reload, whose $BWP re-nocks the arrow"
    );

    // …and settles back into the hold, closing the cycle.
    advance(&mut app, 1000);
    app.update();
    assert_eq!(
        gait(&app),
        Some(109),
        "fire → re-pull → hold, once per shot"
    );

    // The volley ends — the cancel's `RecomputeBaseAnim(-1)`.
    app.world_mut()
        .entity_mut(unit)
        .remove::<crate::creature_anim::AutoRepeatArmed>();
    app.update();
    app.update();
    assert_eq!(
        gait(&app),
        Some(0),
        "dropping the arm recomputes out of the hold and the shooter stands up"
    );
}

/// **B307's driver half** — the link after the router: does an `EmoteAnim { anim_id: 46 }`
/// arriving on a self-player who is [`crate::creature_anim::AutoRepeatArmed`], drawn
/// (`sheath_cur == 2`) and standing in the pull (gait 105) actually ARM clip 46 on the body,
/// **shot after shot**?
///
/// Two guards sit on that path and each could silently eat shots 2 and 3 of a volley — leaving
/// exactly the reported symptom, a shooter that fires from a still pose:
///
/// 1. the **combat fast-path** (`0x5fe43c`): a combat clip requested while another combat clip
///    plays is not armed at all — the live clip doubles rate and the request parks. AttackBow is
///    NOT in the client's combat set (`0x5fcc10`: `10 | 16..=24 | 30 | 36 | 57..=59 | 85..=88 |
///    95 | 117 | 118`), so it must never take this road;
/// 2. the **arm-level same-id dedup** (`0x5fdba0`): a requested id already occupying its slot
///    *and still playing* is not re-armed. Written when 0994's law held the base out of the
///    recompute, so shot 2 would find `Mode::Swing { id: 46 }` still set; decision 1544 restored
///    the recompute, so the mode is back in `Gait` by then. The dedup is checked here either way
///    — it is the guard that would swallow a shot if a fire clip ever were still live.
///
/// Driven through the real `EmoteAnim` lane (no poking `drv.mode`), with real clip assets so
/// Bevy completes them, and 3 s of clock between shots — a bow's cadence.
#[test]
fn every_shot_of_a_volley_re_arms_the_fire_clip_through_the_emote_lane() {
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    /// Stand-in spans for LoadBow and AttackBow. The exact numbers are not load-bearing and
    /// are not claimed to be the real M2's; what matters is only that a fire clip is far
    /// shorter than the ~3 s a bow puts between Auto Shots, so the dedup's "still playing"
    /// test must read false by the next shot.
    const PULL: f32 = 0.7;
    const FIRE: f32 = 0.5;

    let mut app = app();
    let asset = |app: &mut App, secs: f32| {
        let mut c = AnimationClip::default();
        c.set_duration(secs);
        app.world_mut()
            .resource_mut::<Assets<AnimationClip>>()
            .add(c)
    };
    let (stand, pull, fire) = (
        asset(&mut app, 1.0),
        asset(&mut app, PULL),
        asset(&mut app, FIRE),
    );
    let (graph, nodes) = AnimationGraph::from_clips([stand, pull, fire]);
    let graph_handle = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);

    // The shooter's model. `archer_model` deliberately authors no 46; a real HumanMale.m2 does
    // (sequences 46/49/105/106, `benilla-extract … m2seq`), and 46 has to exist for "was it
    // armed?" to be an observable question at all.
    let mut stand_clip = clip(0, 0, true);
    stand_clip.node = nodes[0];
    let mut pull_clip = clip(105, 0, false);
    pull_clip.node = nodes[1];
    pull_clip.duration = PULL;
    let mut fire_clip = clip(46, 0, false);
    fire_clip.node = nodes[2];
    fire_clip.duration = FIRE;

    let unit = app
        .world_mut()
        .spawn((
            ModelAnimations {
                graph: graph_handle.clone(),
                clips: vec![stand_clip, pull_clip, fire_clip],
                hand_close: [None, None],
                playable_animation_lookup: Vec::new(),
                animation_lookup: Vec::new(),
                global_bones: Vec::new(),
                first_seq: None,
                pose: Default::default(),
            },
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
            crate::net::SelfPlayer,
            Wielded {
                ranged: Some((2, 0x2)), // bow
                ..Default::default()
            },
            crate::creature_anim::AutoRepeatArmed,
        ))
        .id();

    // The volley opens: the stance snaps drawn (`SMSG_SPELL_START`'s ranged snap) and the
    // shooter pulls.
    app.world_mut().write_message(SheathRequest {
        entity: unit,
        state: 2,
        ceremony: false,
    });
    app.update();
    app.update();
    let gait = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    let deferred = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .deferred
    };
    let fire_running = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(nodes[2])
            .map(|a| !a.is_finished())
    };
    assert_eq!(gait(&app), Some(105), "the volley opens with the pull");

    const FIRING: super::super::select::Mode = super::super::select::Mode::Swing {
        id: 46,
        under: None,
    };
    for shot in 1..=3u64 {
        // `SMSG_SPELL_GO` → the router's cast kit → this message. Nothing else changes.
        app.world_mut().write_message(EmoteAnim {
            entity: unit,
            anim_id: 46,
            seq: shot,
        });
        app.update();
        assert_eq!(mode(&app), FIRING, "shot {shot} takes bone 0");
        assert_eq!(
            fire_running(&app),
            Some(true),
            "shot {shot}'s release clip is armed and RUNNING — neither the combat fast-path nor \
             the same-id dedup swallowed it"
        );
        assert_eq!(
            deferred(&app),
            None,
            "shot {shot} was a normal arm, not a fast-path park"
        );

        // The ~3 s to the next shot. The clip plays out and CLAMPS on its authored tail: no
        // recompute for a bow id under auto-repeat, so no second pull is armed between shots.
        advance(&mut app, 3000);
        assert_eq!(
            fire_running(&app),
            Some(false),
            "shot {shot}'s clip finished long before the next — which is exactly what keeps the \
             same-id dedup from eating shot {}",
            shot + 1
        );
        assert_eq!(
            mode(&app),
            FIRING,
            "…and the base never recomputed out of it"
        );
        assert_eq!(
            gait(&app),
            None,
            "…so the pull was not replayed between shots"
        );
    }

    // The volley ends — the cancel's `RecomputeBaseAnim(-1)`: the shooter stands up.
    app.world_mut()
        .entity_mut(unit)
        .remove::<crate::creature_anim::AutoRepeatArmed>();
    app.update();
    app.update();
    assert_eq!(
        gait(&app),
        Some(0),
        "dropping the arm stands the shooter up"
    );
}

/// A vendor built on **real Bevy clip assets**, not the stand-in `clip()` handles the rest of this
/// file uses: this tenant is about a window *completing*, and `completions()` only ticks for an
/// entity whose graph is a real asset (`advance_animations` skips the others — see [`app`]). The
/// timings are the shipped HumanMale's (`benilla-extract m2seq`): Stand 2.667 s / blend 0.500 s,
/// ShuffleLeft & ShuffleRight 0.500 s / blend 0.250 s, and `replay = (0,0)` on both shuffles —
/// which is what every one of the 1130 shipped shuffle records carries, so `R` is a deterministic
/// 1 and the window is exactly one span.
///
/// Returns the entity plus the three graph nodes, in id order: Stand, ShuffleLeft, ShuffleRight.
fn spawn_vendor(app: &mut App) -> (Entity, Vec<AnimationNodeIndex>) {
    use benilla_protocol::EntityKind;
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    let handles: Vec<_> = [2.667f32, 0.5, 0.5]
        .iter()
        .map(|d| {
            let mut c = AnimationClip::default();
            c.set_duration(*d);
            app.world_mut()
                .resource_mut::<Assets<AnimationClip>>()
                .add(c)
        })
        .collect();
    let (graph, nodes) = AnimationGraph::from_clips(handles);
    let graph_handle = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);
    let seq = |id: u16, node, duration: f32, blend_time: f32| AnimClip {
        node,
        duration,
        blend_time,
        replay: (0, 0),
        ..clip(id, 0, true)
    };
    let anims = ModelAnimations {
        graph: graph_handle.clone(),
        clips: vec![
            seq(0, nodes[0], 2.667, 0.5),
            seq(11, nodes[1], 0.5, 0.25),
            seq(12, nodes[2], 0.5, 0.25),
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    // The vendor five yards east of the world origin, authored facing +x — its back to a player
    // standing north-west of it, which is the reporter's own screenshot in B110.
    let npc = app
        .world_mut()
        .spawn((
            anims,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
            crate::net::NetEntity {
                kind: EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            crate::net::ObjectStore::default(),
            Transform {
                translation: benilla_assets::coords::wow_to_bevy([5.0, 0.0, 0.0]),
                rotation: Quat::from_rotation_y(0.0),
                ..default()
            },
        ))
        .id();
    (npc, nodes)
}

/// The **cross-seam** test for the turn-shuffle: `net::motion::facing` produces the
/// [`crate::net::FacingStep`] latch and this driver consumes it, and until this test the two had
/// only ever been exercised apart — the producer's tests read the transform, the consumer's tests
/// inserted the component by hand. Neither could see the seam, which is how a mechanism that was
/// all present and firing produced nothing anyone could see: "when interacting with a vendor the
/// NPC doesn't shuffle its feet, it turns frozen" (director, 2026-08-27).
///
/// Two laws meet here, and the test is written so that failing either one is legible:
///
/// - **the latch runs the ease out** — the client tests the yaw its pump APPLIED against ±1e-5,
///   not the gap remaining against an eyeballed 3°, so the shuffle blends in properly instead of
///   being handed back at 0.32 weight;
/// - **the shuffle is released by its own clip window, never by the turn ending** — `0x607ed0`'s
///   tail can only start one (`0x5fce30`'s gate needs a turn bit, which is exactly what going
///   still clears), so what ends it is the completion callback one span later. That is why the
///   reference plays a discrete little step and not a stub of one, and it is why the middle
///   assertion here is that the feet are STILL moving long after the body has stopped.
#[test]
fn the_interaction_face_me_shuffles_its_feet_for_the_whole_turn() {
    let mut app = app();
    app.init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::ui_session::InteractNpc>();
    app.add_systems(
        Update,
        crate::net::drive_display_facing.before(drive_animations),
    );
    // Us off the vendor's axis — a bearing of ~2.60 rad, the representative case. (A goal at
    // exactly ±pi sits on the yaw wrap, where the client's own unfolded-delta dead-band never
    // snaps and the ease runs to float underflow instead; `net::motion::facing`'s tests say the
    // same thing about the same corner.)
    app.world_mut().spawn((
        crate::net::SelfPlayer,
        crate::net::ActiveMover,
        Transform::from_translation(benilla_assets::coords::wow_to_bevy([0.0, 3.0, 0.0])),
    ));
    let (npc, nodes) = spawn_vendor(&mut app);

    let gait = |app: &App| app.world().entity(npc).get::<AnimDriver>().unwrap().gait;
    let turning = |app: &App| app.world().entity(npc).contains::<crate::net::FacingStep>();
    // How heavily either shuffle is being blended in — the "is it actually on screen" number.
    let shuffle_weight = |app: &App| {
        let p = app.world().entity(npc).get::<AnimationPlayer>().unwrap();
        nodes[1..]
            .iter()
            .filter_map(|n| p.animation(*n).map(|a| a.weight()))
            .fold(0.0f32, f32::max)
    };
    // Run `ms` of 16 ms frames, reporting the heaviest the shuffle got and whether it held the
    // gait slot throughout.
    let run = |app: &mut App, ms: u64, want: u16| {
        let (mut peak, mut held) = (0.0f32, true);
        for _ in 0..(ms / 16) {
            advance(app, 16);
            peak = peak.max(shuffle_weight(app));
            held &= gait(app) == Some(want);
        }
        (peak, held)
    };

    // The control: nothing open, nothing turning — the vendor stands, and keeps standing.
    let (peak, _) = run(&mut app, 128, 0);
    assert_eq!(gait(&app), Some(0), "no window, no turn");
    assert_eq!(peak, 0.0, "and no shuffle to be seen");

    // Open its window. It turns to us — and its feet move while it does. 256 ms is past the ease
    // (this bearing closes in ten pumps ~= 160 ms) and past the shuffle's own 250 ms blend-in.
    app.world_mut()
        .resource_mut::<crate::ui_session::InteractNpc>()
        .0 = Some(npc);
    let (peak, held) = run(&mut app, 256, 11);
    assert!(held, "the whole turn is ShuffleLeft, got {:?}", gait(&app));
    assert!(
        peak > 0.95,
        "the shuffle blends the whole way in; the frozen-feet defect peaked at 0.32, got {peak}"
    );

    // **The body has stopped and the feet have not.** The ease settled ~100 ms ago; the shuffle is
    // held to its 500 ms window, which is what makes it a step rather than a twitch. A driver that
    // released on the settle fails here — and so does one that released on the latch, one frame
    // after the yaw went quiet.
    assert!(!turning(&app), "the ease has settled");
    let (_, held) = run(&mut app, 192, 11);
    assert!(
        held,
        "the shuffle is held past the settle to its own window, got {:?}",
        gait(&app)
    );

    // …and released AT that window, not never. `0x5fc3f0`'s completion row is a
    // `RecomputeBaseAnim(-1)`, so the chain now answers Stand — with a freshly rolled variation,
    // which is the idle fidget of 0123.
    run(&mut app, 160, 0);
    assert_eq!(
        gait(&app),
        Some(0),
        "the completed window returns it to Stand"
    );
    // Let the idle finish arriving before the next half — Stand's own blend is 500 ms, and a
    // window closed mid-fade would put THREE tracks on the slot at once (the retiring shuffle,
    // the half-arrived Stand, the new shuffle) and split the weight three ways. That is a real
    // divergence we carry knowingly — the reference's `0x7125d4` refuses to re-seed a blend that
    // has not passed half weight (1565 §6, 1570) — but it is not what this test is about, and a
    // player browsing a vendor's stock is well past it either way.
    run(&mut app, 640, 0);

    // Close it. The goal falls back to the untouched wire facing and it swings home — "and again
    // when it turns back" (the report's second half), so the feet move for that turn too, on the
    // other side.
    app.world_mut()
        .resource_mut::<crate::ui_session::InteractNpc>()
        .0 = None;
    let (peak, held) = run(&mut app, 256, 12);
    assert!(held, "the swing back is ShuffleRight, got {:?}", gait(&app));
    assert!(peak > 0.95, "and blends the whole way in: {peak}");
}

/// **A disarmed attacker fights bare-handed** — the whole of `UNIT_FLAG_DISARMED`
/// on the animation side, run through the real driver against an armed control on the same frame.
/// The reference gets here with no disarm case in any selector: `GetWeapon(slot, 0)` hands the
/// swing (`0x6246a0`) and the Ready idle (`0x5fcdc0`) a NULL hand, and their existing unarmed legs
/// do the rest. So the pass is *unarmed clips*, and the failure this pins is the sword's Attack1H
/// coming out of a hand the server says is empty.
#[test]
fn a_disarmed_attacker_swings_and_stands_unarmed() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, 1, true),    // Stand
                clip(17, 2, false),  // Attack1H — the sword swing
                clip(16, 3, false),  // AttackUnarmed — the fist
                clip(26, 4, true),   // Ready1H — the sword stance
                clip(25, 5, true),   // ReadyUnarmed — the bare-handed stance
                clip(88, 6, false),  // AttackOffPierce — the offhand dagger
                clip(117, 7, false), // AttackUnarmedOff — the offhand fist
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }
    // Sword and dagger, engaged: the same loadout twice, differing only in the descriptor bit.
    fn fighter(app: &mut App, disarmed: bool) -> Entity {
        app.world_mut()
            .spawn((
                model(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimDriver::default(),
                Engaged(0),
                Wielded {
                    main: Some((2, 0x7)), // 1H sword
                    off: Some((2, 0xf)),  // dagger
                    main_sheath: 3,
                    off_sheath: 3,
                    disarmed,
                    ..Default::default()
                },
            ))
            .id()
    }
    let gait = |app: &App, unit: Entity| app.world().entity(unit).get::<AnimDriver>().unwrap().gait;
    let playing = |app: &App, unit: Entity, node: u32| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(AnimationNodeIndex::new(node as usize))
            .is_some()
    };

    // The engaged standing idle: `0x5fcdc0`'s weapon-class Ready, or ReadyUnarmed for the hand
    // the disarm emptied.
    let mut stand = app();
    let armed = fighter(&mut stand, false);
    let disarmed = fighter(&mut stand, true);
    stand.update();
    assert_eq!(gait(&stand, armed), Some(26), "the sword stands Ready1H");
    assert_eq!(
        gait(&stand, disarmed),
        Some(25),
        "the disarmed hand stands ReadyUnarmed, sword or no sword"
    );

    // One swing per hand, each on its own pair — a second swing over a live one is the combat
    // fast-path's deferral (`0x5fcc10`), which has nothing to say about disarm.
    let swings = |hit_info: u32| {
        let mut app = app();
        let armed = fighter(&mut app, false);
        let disarmed = fighter(&mut app, true);
        app.update();
        for (unit, seq) in [(armed, 1u64), (disarmed, 2)] {
            app.world_mut().write_message(SwingMessage {
                attacker: unit,
                victim: None,
                hit_info,
                victim_state: 1,
                damage: 7,
                displayed: true,
                seq,
            });
        }
        app.update();
        let nodes = |unit| {
            (1..8u32)
                .filter(|n| playing(&app, unit, *n))
                .collect::<Vec<_>>()
        };
        (nodes(armed), nodes(disarmed))
    };

    // The MAINHAND swing (`0x6246a0`, HitInfo bit 0x4 clear): Attack1H(17) vs AttackUnarmed(16).
    let (armed_nodes, disarmed_nodes) = swings(0);
    assert!(
        armed_nodes.contains(&2),
        "the armed control swings Attack1H: {armed_nodes:?}"
    );
    assert!(
        disarmed_nodes.contains(&3) && !disarmed_nodes.contains(&2),
        "the disarmed attacker swings AttackUnarmed(16), never the sword's clip: {disarmed_nodes:?}"
    );

    // …and the OFFHAND swing (HitInfo bit 0x4). This is the half the ladder decides: the
    // off-hand gate's FIRST probe asks about the MAIN hand, and a main hand holding a weapon
    // CANCELS it (`5ec28d je 0x5ec2aa`). A disarmed dual-wielder therefore keeps stabbing with
    // the dagger — disarm hides exactly one weapon.
    let (armed_nodes, disarmed_nodes) = swings(0x4);
    assert!(
        armed_nodes.contains(&6),
        "the armed control stabs AttackOffPierce: {armed_nodes:?}"
    );
    assert!(
        disarmed_nodes.contains(&6) && !disarmed_nodes.contains(&7),
        "the disarmed off hand KEEPS its dagger — 88, not 117: {disarmed_nodes:?}"
    );
}

/// The other rung of the ladder: with **no weapon in the main hand** the disarm falls to the off
/// hand, and that — not the dual-wield case — is what reaches AttackUnarmedOff(117) (decision
/// 1863; the swing selector `0x6246a0`).
#[test]
fn an_off_hand_only_fighter_is_the_case_that_punches_off_hand() {
    let mut app = app();
    let unit = app
        .world_mut()
        .spawn((
            ModelAnimations {
                graph: Handle::default(),
                clips: vec![
                    clip(0, 1, true),
                    clip(16, 3, false),  // AttackUnarmed
                    clip(88, 6, false),  // AttackOffPierce
                    clip(117, 7, false), // AttackUnarmedOff
                ],
                hand_close: [None, None],
                playable_animation_lookup: Vec::new(),
                animation_lookup: Vec::new(),
                global_bones: Vec::new(),
                first_seq: None,
                pose: Default::default(),
            },
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            Wielded {
                main: None,          // nothing in the main hand to claim the disarm
                off: Some((2, 0xf)), // …so the dagger is the weapon it hides
                disarmed: true,
                ..Default::default()
            },
        ))
        .id();
    app.update();
    app.world_mut().write_message(SwingMessage {
        attacker: unit,
        victim: None,
        hit_info: 0x4,
        victim_state: 1,
        damage: 7,
        displayed: true,
        seq: 1,
    });
    app.update();
    let playing = |node: u32| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(AnimationNodeIndex::new(node as usize))
            .is_some()
    };
    assert!(playing(7), "AttackUnarmedOff(117)");
    assert!(!playing(6), "not the dagger's own clip");
}

/// The **play-time** substitution ([`super::super::select::unarmed_special`], `0x5fe2f0`): a
/// spell kit's Special1H(57) becomes SpecialUnarmed(118) when both hands read empty — which a
/// disarmed dual-wielder is NOT (it keeps its off hand), and a disarmed single-wielder is.
#[test]
fn a_special_goes_unarmed_only_when_both_hands_are_empty() {
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),
            clip(57, 2, false),  // Special1H — the kit's weapon spin
            clip(118, 3, false), // SpecialUnarmed
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    };
    let mut app = app();
    let spin = |app: &mut App, w: Wielded| {
        let unit = app
            .world_mut()
            .spawn((
                model(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimDriver::default(),
                w,
            ))
            .id();
        app.update();
        app.world_mut().write_message(EmoteAnim {
            entity: unit,
            anim_id: 57,
            seq: 1,
        });
        unit
    };
    let armed = spin(
        &mut app,
        Wielded {
            main: Some((2, 0xf)),
            ..Default::default()
        },
    );
    let dual = spin(
        &mut app,
        Wielded {
            main: Some((2, 0x7)),
            off: Some((2, 0xf)),
            disarmed: true,
            ..Default::default()
        },
    );
    let alone = spin(
        &mut app,
        Wielded {
            main: Some((2, 0x7)),
            disarmed: true,
            ..Default::default()
        },
    );
    app.update();
    let playing = |app: &App, unit: Entity, node: u32| {
        app.world()
            .entity(unit)
            .get::<AnimationPlayer>()
            .unwrap()
            .animation(AnimationNodeIndex::new(node as usize))
            .is_some()
    };
    assert!(playing(&app, armed, 2), "control: the weapon spin");
    assert!(
        playing(&app, dual, 2),
        "a disarmed dual-wielder still has a hand full — the weapon spin"
    );
    assert!(
        playing(&app, alone, 3) && !playing(&app, alone, 2),
        "both hands empty to the gate — SpecialUnarmed(118)"
    );
}

/// A creature whose template carries **DO_NOT_PLAY_WOUND_ANIM** (`type_flags & 0x8`) takes no
/// wound flinch at all — the reference's `0x60ea9f` gate inside the flinch itself, so it covers
/// every trigger: the melee hit, the `$HIT` echo and the spell-side severity-0 call alike.
/// This is the skeleton case: no flesh, no recoil.
///
/// The control beside it is the whole point — the gate must key on bit `0x8` and nothing else,
/// and benilla read the *neighbouring* bit for months. So the unflagged victim here carries
/// `NO_FACTION_TOOLTIP | MORE_AUDIBLE` (`0x30`, the two bits we already consume) and must still
/// flinch: a gate on either neighbour, or on "any flag at all", fails right here.
#[test]
fn a_no_wound_creature_takes_no_flinch() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, 1, true),  // Stand
                clip(8, 2, false), // StandWound — the unengaged victim's severity-0 pick
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }
    // The victim's descriptor, carrying the one field the gate keys on.
    fn streamed(entry: u32) -> crate::net::ObjectStore {
        crate::net::ObjectStore(ObjectFields::from_pairs(&[(OBJECT_FIELD_ENTRY, entry)]))
    }
    const OBJECT_FIELD_ENTRY: u16 = 3;
    const SKELETON: u32 = 1783; // a Scarlet Monastery skeleton's template entry
    const WOLF: u32 = 69;

    let mut app = app();
    let record = |type_flags: u32| crate::names::CreatureRecord {
        name: "victim".into(),
        subname: None,
        creature_type: 6, // Undead
        pet_family: 0,
        rank: 0,
        type_flags,
        civilian: false,
        racial_leader: false,
        display_id: 0,
    };
    {
        let mut names = app.world_mut().resource_mut::<crate::names::NameCache>();
        names.insert_creature(SKELETON, Some(record(DO_NOT_PLAY_WOUND_ANIM)));
        // The control's flags are the two NEIGHBOURS, both set.
        names.insert_creature(WOLF, Some(record(NO_FACTION_TOOLTIP | MORE_AUDIBLE)));
    }

    let spawn = |app: &mut App, entry: u32| {
        app.world_mut()
            .spawn((
                model(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimDriver::default(),
                streamed(entry),
            ))
            .id()
    };
    let skeleton = spawn(&mut app, SKELETON);
    let wolf = spawn(&mut app, WOLF);
    // A creature whose query has not answered yet: the reference's null-record leg passes, so it
    // flinches like anything else.
    let unqueried = spawn(&mut app, 4242);
    app.update(); // settle every base on Stand

    for e in [skeleton, wolf, unqueried] {
        app.world_mut().write_message(WoundAnim { entity: e });
    }
    app.update();

    let wound = |e: Entity| {
        app.world()
            .entity(e)
            .get::<AnimDriver>()
            .unwrap()
            .wound
            .is_some()
    };
    assert!(
        !wound(skeleton),
        "DO_NOT_PLAY_WOUND_ANIM refuses the flinch"
    );
    assert!(
        wound(wolf),
        "the neighbouring bits (0x10 / 0x20) must not gate the flinch"
    );
    assert!(
        wound(unqueried),
        "a template we have not received reads as unflagged — `0x6125f0`'s null leg"
    );
}

/// The flag's **second** consumer: `DO_NOT_PLAY_WOUND_ANIM` takes the victim's **parry** with its
/// flinch (`0x60ec1f` inside the parry pick `0x60ec00` — the bit has exactly two callers and this
/// is the other one). The `$CPP` ladder enters `0x60ec00` only on victimState 3, so DODGE and BLOCK
/// reach `PlayAnimation` directly and are **not** gated: a flagged creature still dodges, it just
/// never parries. Both halves are asserted here — the gate without its control is the bug this
/// whole record is about.
#[test]
fn the_no_wound_flag_takes_the_parry_but_not_the_dodge() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, 1, true),   // Stand
                clip(21, 2, false), // Parry1H — a 1H sword's parry pick
                clip(30, 3, false), // Dodge
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }
    const OBJECT_FIELD_ENTRY: u16 = 3;
    const FLAGGED: u32 = 3870; // Stone Sleeper — really carries the bit on our world DB
    const PLAIN: u32 = 69;

    let mut app = app();
    let record = |type_flags: u32| crate::names::CreatureRecord {
        name: "victim".into(),
        subname: None,
        creature_type: 6,
        pet_family: 0,
        rank: 0,
        type_flags,
        civilian: false,
        racial_leader: false,
        display_id: 0,
    };
    {
        let mut names = app.world_mut().resource_mut::<crate::names::NameCache>();
        names.insert_creature(FLAGGED, Some(record(DO_NOT_PLAY_WOUND_ANIM)));
        names.insert_creature(PLAIN, Some(record(0)));
    }
    let spawn = |app: &mut App, entry: u32| {
        app.world_mut()
            .spawn((
                model(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimDriver::default(),
                // A 1H sword in the mainhand: `defense_anim` sends class 2 subclass 7 to
                // Parry1H(21).
                Wielded {
                    main: Some((2, 7)),
                    ..Default::default()
                },
                crate::net::ObjectStore(ObjectFields::from_pairs(&[(OBJECT_FIELD_ENTRY, entry)])),
            ))
            .id()
    };
    let flagged = spawn(&mut app, FLAGGED);
    let plain = spawn(&mut app, PLAIN);
    let dodger = spawn(&mut app, FLAGGED);
    app.update(); // settle every base on Stand

    for (victim, victim_state) in [(flagged, 3), (plain, 3), (dodger, 2)] {
        app.world_mut()
            .write_message(crate::creature_anim::DefenseAnim {
                victim,
                victim_state,
            });
    }
    app.update();

    let base = |e: Entity| {
        app.world()
            .entity(e)
            .get::<AnimationTransitions>()
            .unwrap()
            .get_main_animation()
            .map(|n| n.index())
    };
    assert_eq!(
        base(flagged),
        Some(1),
        "flagged: no parry — the base holds Stand"
    );
    assert_eq!(base(plain), Some(2), "unflagged: Parry1H(21) plays");
    assert_eq!(
        base(dodger),
        Some(3),
        "the flag does not reach DODGE — it enters `0x60ec00` only on victimState 3"
    );
}

/// **The weapon-trail latch's edge**: `AnimDriver::started_anim` must be raised by
/// a plain gait change, not only by a one-shot.
///
/// `0x5fe2f0` is the image's single animation entry point — locomotion reaches it through
/// `0x602c60` → `0x5fd9e0` → `0x5fd8b0` → `0x5fd100`, and no locomotion id is in the combat set
/// that takes its no-latch fast path — so a unit that simply starts running consumes the pending
/// arm. That is what starts Charge's trail: kit 44's anim id is `-1`, `0x60f366 jl` skips the play
/// block entirely, and the arm waits for the charge's own run. A one-shot-only edge would leave 38
/// Charge spells with no trail at all.
///
/// The other half matters as much: there is **no per-frame animation recompute** in the reference
/// (`0x5fd8b0` has one caller and `0x5fd9e0`'s 38 sites are all event-driven), so a unit standing
/// still must NOT keep raising the edge and eating arms.
#[test]
fn a_gait_change_raises_the_anim_edge_and_a_steady_frame_does_not() {
    let mut app = app();
    let unit = app
        .world_mut()
        .spawn((
            caster_model(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimDriver::default(),
            MovementState::default(),
        ))
        .id();
    let edge = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .started_anim()
    };
    app.update(); // settle: Stand — itself a play
    app.update();
    assert!(!edge(&app), "a settled, motionless unit plays nothing");
    // Start running: the gait changes, so the base track takes a play.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    assert!(edge(&app), "starting to run IS a PlayAnimation");
    app.update();
    assert!(
        !edge(&app),
        "…and holding that run is not — the reference has no per-frame recompute"
    );
    // A one-shot raises it too, which is the ordinary case.
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 57,
        seq: 1,
    });
    app.update();
    assert!(edge(&app), "and so does a one-shot");
}

/// **The combat fast path must NOT raise the anim edge**.
///
/// `0x5fe43c` returns at `0x5fe48b` — *before* `0x5fe48e`, the weapon-trail latch's only read —
/// when the unit is already playing a combat animation and requests another one. So a pending
/// trail arm survives that play. It matters because `0x60d835` arms a **one-slot** field with a
/// plain `mov`: a second proc overwrites the first before it ever fires, and a client that fired
/// on every request would draw trails from arms the reference superseded — with the superseded
/// colours and durations, on 23 of the 34 type-8 kits.
///
/// benilla does not need to build that gate: the driver's request loop already fast-paths
/// combat-over-combat and `continue`s without playing, so the edge never rises.
/// This pins the connection between the two, which is otherwise invisible — they live in different
/// modules and neither mentions the other's mechanism.
#[test]
fn a_combat_over_combat_fast_path_does_not_raise_the_anim_edge() {
    let mut app = app();
    let unit = jumper(&mut app);
    let edge = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .started_anim()
    };
    let parked = |app: &App| {
        app.world()
            .entity(unit)
            .get::<AnimDriver>()
            .unwrap()
            .deferred
    };
    app.update(); // settle: Stand
                  // A combat one-shot — Special1H(57) is in `0x5fcc10`'s set, and the model has it.
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 57,
        seq: 1,
    });
    app.update();
    assert!(edge(&app), "the first combat play arms normally");
    // A SECOND combat request while the first still runs — AttackUnarmed(16), also in the set.
    // The fast path re-times the live clip and parks the request; nothing is armed.
    app.world_mut().write_message(SwingMessage {
        attacker: unit,
        victim: None,
        hit_info: 0x2,
        victim_state: 1,
        damage: 21,
        displayed: true,
        seq: 2,
    });
    app.update();
    assert_eq!(
        parked(&app),
        Some(16),
        "the swing parks behind the kit clip — the fast path fired"
    );
    assert!(
        !edge(&app),
        "…and it re-times rather than plays, so `0x5fe2f0` returns before the latch read and a \
         pending trail arm survives"
    );
}

/// The **base-animation lock** —
/// the reason a Lashed player visibly falls over, and the correction to 2085's reading.
///
/// A stun's root recomputes the base unconditionally and the selector resolves `Stand(0)`; what
/// stops that overwriting the victim's `Knockdown` is `0x5fe2f0`'s head guard on
/// `[unit+0xd58] & 0xc0000`, a bit the arm helper set keyed on the id it actually armed. So the
/// recompute is not skipped — **the play it asks for is refused**, and the clip runs its full
/// 2000 ms. The director watched exactly this on the reference client, fighting the Silithus worm
/// whose Lash (6607) puts nothing on its victim but that one clip.
///
/// The same guard scopes 2085's claim that a state kit's anim is an animation *cutter*: it cannot
/// cut a clip that took the lock, so Charge does **not** cut its own Knockdown.
mod base_anim_lock {
    use super::*;
    use crate::creature_anim::{BaseAnimRecompute, Mode};

    /// Stand, Knockdown (the locking id), and SpecialUnarmed (an ordinary one-shot that does not
    /// lock) — the pair that separates the guard from a plain re-pick.
    const STAND_NODE: u32 = 1;
    const KNOCKDOWN_NODE: u32 = 2;

    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, STAND_NODE, true),        // Stand
                clip(121, KNOCKDOWN_NODE, false), // Knockdown — takes the lock
                clip(118, 3, false),              // SpecialUnarmed — takes nothing
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    fn victim(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                model(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimDriver::default(),
            ))
            .id()
    }

    fn main_node(app: &App, unit: Entity) -> Option<AnimationNodeIndex> {
        app.world()
            .entity(unit)
            .get::<AnimationTransitions>()
            .unwrap()
            .get_main_animation()
    }

    fn oneshot(app: &mut App, unit: Entity, anim_id: u16) {
        app.world_mut().write_message(EmoteAnim {
            entity: unit,
            anim_id,
            seq: 1,
        });
        app.update();
    }

    fn recompute(app: &mut App, unit: Entity, anim_id: u16) {
        app.world_mut().write_message(BaseAnimRecompute {
            entity: unit,
            anim_id,
        });
        app.update();
    }

    /// The whole report, in one assertion: a `Knockdown` on the base survives the recompute the
    /// stun's own root triggers, because the `Stand` it asks for is refused.
    #[test]
    fn a_knockdown_survives_the_base_recompute() {
        let mut app = app();
        let unit = victim(&mut app);
        oneshot(&mut app, unit, 121);
        assert_eq!(
            main_node(&app, unit),
            Some(AnimationNodeIndex::new(KNOCKDOWN_NODE as usize)),
            "the impact kit's Knockdown holds the base"
        );

        recompute(&mut app, unit, 14); // the state kit's `Stun`, which is never played

        assert_eq!(
            main_node(&app, unit),
            Some(AnimationNodeIndex::new(KNOCKDOWN_NODE as usize)),
            "the recompute fires, resolves Stand, and the lock refuses it — the clip stays"
        );
        assert_eq!(
            app.world().entity(unit).get::<AnimDriver>().unwrap().gait,
            None,
            "and nothing may claim the base holds Stand: the target stays unset so the selector \
             tries again once the clip releases the lock"
        );
    }

    /// …and the guard is the id's, not a blanket refusal: an ordinary one-shot takes no lock, so
    /// the same recompute ends it. This is 2085's mechanism, correctly scoped.
    #[test]
    fn a_non_locking_one_shot_is_cut_by_the_same_recompute() {
        let mut app = app();
        let unit = victim(&mut app);
        oneshot(&mut app, unit, 118);
        assert!(
            matches!(
                app.world().entity(unit).get::<AnimDriver>().unwrap().mode,
                Mode::Swing { id: 118, .. }
            ),
            "SpecialUnarmed holds the base slot"
        );

        recompute(&mut app, unit, 14);

        assert_eq!(
            main_node(&app, unit),
            Some(AnimationNodeIndex::new(STAND_NODE as usize)),
            "nothing refused the Stand, so it took the slot"
        );
    }

    /// A state kit naming the id already playing does nothing at all (`0x60f393 je 0x60f3ca`).
    #[test]
    fn a_matching_state_kit_anim_leaves_the_base_alone() {
        let mut app = app();
        let unit = victim(&mut app);
        oneshot(&mut app, unit, 118);
        recompute(&mut app, unit, 118);
        assert!(
            matches!(
                app.world().entity(unit).get::<AnimDriver>().unwrap().mode,
                Mode::Swing { id: 118, .. }
            ),
            "already playing it — the leg leaves the block having done nothing"
        );
    }

    /// The airborne clips, on the same victim — the director's follow-up report: *"there is some
    /// bug it seems, I ended up stuck laying down after a knock, might be because I was jumping
    /// or running at the same time"*.
    fn airborne_model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, STAND_NODE, true),        // Stand
                clip(121, KNOCKDOWN_NODE, false), // Knockdown — takes the lock
                clip(37, 3, false),               // JumpStart
                clip(38, 4, true),                // Jump hang
                clip(39, 5, false),               // JumpEnd
                clip(40, 6, true),                // Fall
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    fn knocked_jumper(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                airborne_model(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimDriver::default(),
                MovementState::default(),
            ))
            .id()
    }

    /// **The wedge**: jumping while the lock is held used to stop the locked clip DEAD, and a clip
    /// that never finishes never releases the lock — so the body lay on its back for the rest of
    /// the session, every subsequent play refused.
    ///
    /// The mechanism is 0503's snapshot-freeze in [`super::play::leave_special`], which stills the
    /// cut airborne clip before the landing cross-fades over it. It took whatever bone 0 held, on
    /// the assumption that the arc's own clip is what it armed — and the lock is the first thing
    /// ever to falsify that: every play the arc asked for was refused, so bone 0 still held the
    /// `Knockdown`, and the landing froze *that*.
    #[test]
    fn a_jump_taken_while_locked_never_freezes_the_locked_clip() {
        let mut app = app();
        let unit = knocked_jumper(&mut app);
        app.update(); // settle: Stand
        oneshot(&mut app, unit, 121);
        let knockdown = AnimationNodeIndex::new(KNOCKDOWN_NODE as usize);
        assert_eq!(
            main_node(&app, unit),
            Some(knockdown),
            "the impact kit's Knockdown holds the base"
        );

        // Space, flat on their back: the launch enters the bracket, and every clip in the bracket
        // is refused — so bone 0 still holds the Knockdown for the whole arc.
        app.world_mut().entity_mut(unit).insert(MovementState {
            flags: move_flags::FALLING,
            vertical_speed: 7.96,
            ..Default::default()
        });
        advance(&mut app, 16);
        assert!(
            !app.world()
                .entity(unit)
                .get::<AnimDriver>()
                .unwrap()
                .started_anim,
            "the bracket walked with its play declined — nothing started, so nothing downstream              may read a play out of the mode change (the flinch's eviction, 2076's trail edge)"
        );
        advance(&mut app, 16);
        assert_eq!(
            main_node(&app, unit),
            Some(knockdown),
            "the arc's own JumpStart/hang are refused, exactly as the guard orders"
        );

        // Touchdown.
        app.world_mut()
            .entity_mut(unit)
            .insert(MovementState::default());
        advance(&mut app, 16);

        assert_eq!(
            app.world().entity(unit).get::<AnimDriver>().unwrap().frozen,
            None,
            "the landing's freeze is the AIRBORNE clip's alone — it must never still a clip the \
             arc did not arm"
        );
        assert_eq!(
            app.world()
                .entity(unit)
                .get::<AnimationPlayer>()
                .unwrap()
                .animation(knockdown)
                .map(bevy::animation::ActiveAnimation::speed),
            Some(1.0),
            "…so the Knockdown runs on, finishes, and releases the lock"
        );
    }
}
