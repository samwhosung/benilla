//! Headless tests of [`super::drive_animations`] on synthetic units, across frames.

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

/// A held non-weapon keeps Special1H(57): the substitution (`0x5fe3cc`) keys on an empty hand.
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

/// A staff caster: Stand, Run, the staff Ready idle and the precast hold, with a masked variant.
fn caster_model() -> ModelAnimations {
    let mut hold = clip(51, 3, true); // ReadySpellDirected, the precast hold
    hold.upper_node = Some(AnimationNodeIndex::new(5));
    ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),  // Stand
            clip(28, 2, true), // Ready2HL, the staff Ready idle
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

/// The real 5875 rows: ReadySpellDirected's WeaponFlags force-stow (`&4`), Ready2HL's and
/// Attack1H's force-draw (`&0x20`).
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

/// A bare `app.update()`'s clock step: nonzero, since a zero delta never ticks a transition.
const FRAME_STEP: std::time::Duration = std::time::Duration::from_millis(1);

/// The next frame's delta when a test asks for one; a resource, because `Time::advance_by` sets
/// the delta rather than adding to it.
#[derive(Resource, Default)]
struct NextStep(Option<std::time::Duration>);

fn step_clock(mut time: ResMut<Time>, mut next: ResMut<NextStep>) {
    time.advance_by(next.0.take().unwrap_or(FRAME_STEP));
}

/// Runs one frame whose delta is exactly `ms`.
fn advance(app: &mut App, ms: u64) {
    app.world_mut().resource_mut::<NextStep>().0 = Some(std::time::Duration::from_millis(ms));
    app.update();
}

fn app() -> App {
    let mut app = App::new();
    // The client's one `rand()` stream: the variation and replay rolls.
    app.init_resource::<benilla_assets::AnimRng>();
    // Completions tick only for units with a real graph asset. `TimePlugin` is off and the clock
    // is `step_clock`: a real-time delta lets a stalled frame run a whole fade out.
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
    // A dead-letter net channel; the driver tolerates the dropped receiver.
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    app.init_resource::<crate::names::NameCache>();
    app.insert_resource(catalog());
    app.add_systems(Update, drive_animations);
    app
}

/// vmangos sets every creature's sheath to melee at spawn (`Creature.cpp:605`), so the hold clip's
/// force-stow is what stows a caster's staff.
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

    app.update();
    assert_eq!(sheath(&app), Some(1), "engaged Ready idle draws");

    app.world_mut().entity_mut(unit).insert(CastHold {
        ranged: false,
        anim_id: 51,
        spell_id: 20793,
    });
    app.update();
    assert_eq!(sheath(&app), Some(0), "the cast hold stows the staff");

    app.world_mut().entity_mut(unit).remove::<CastHold>();
    app.update();
    assert_eq!(sheath(&app), Some(1), "drawn again once the cast resolves");
}

/// The reconcile runs only inside `PlayAnimation` (`0x5fdf80`), so a playless frame keeps the stow.
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

    app.update();
    assert_eq!(sheath(&app), Some(1), "engaged runner draws");

    app.world_mut().entity_mut(unit).insert(CastHold {
        ranged: false,
        anim_id: 51,
        spell_id: 20793,
    });
    app.update();
    assert_eq!(sheath(&app), Some(0), "the masked hold's retake stows");

    for _ in 0..3 {
        app.update();
        assert_eq!(sheath(&app), Some(0), "no play — the stow persists");
    }

    app.world_mut().entity_mut(unit).remove::<CastHold>();
    app.update();
    assert_eq!(sheath(&app), Some(0), "released mid-run — no play yet");

    app.world_mut().entity_mut(unit).remove::<MovementState>();
    app.update();
    assert_eq!(sheath(&app), Some(1), "the stop's Ready play re-draws");
}

/// A fidgeter on real clip assets: Stand's zero-frequency head and max-frequency look-around (the
/// LCG's first roll, 38, lands on it) and a short ShuffleLeft. Nodes: head, look, shuffle.
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
            seq(0, nodes[0], STAND_SPAN, 0),     // Stand, the head
            seq(0, nodes[1], STAND_SPAN, 32767), // Stand, the rare look-around
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

/// A relaxed arm rolls its variation (`variationIdx = −1`), and the shuffle, released at its own
/// window's end since `0x607ed0`'s tail cannot stop it (`0x5fce30`), re-rolls Stand.
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

    app.update();
    assert_eq!(gait(&app), Some(0));
    assert!(active(&app, nodes[1]), "the rolled variation is what armed");
    assert!(!active(&app, nodes[0]), "not the head");

    app.world_mut()
        .entity_mut(unit)
        .insert(crate::net::FacingStep(0.3));
    app.update();
    assert_eq!(gait(&app), Some(11), "stepping yaw → ShuffleLeft");

    app.world_mut()
        .entity_mut(unit)
        .remove::<crate::net::FacingStep>();
    advance(&mut app, 25);
    assert_eq!(
        gait(&app),
        Some(11),
        "the settle does not release the shuffle (1655)"
    );

    for _ in 0..6 {
        advance(&mut app, 25);
    }
    assert_eq!(gait(&app), Some(0), "the completed window → back to Stand");
    assert!(
        active(&app, nodes[0]) || active(&app, nodes[1]),
        "some Stand variation re-armed"
    );
}

/// An engaged unit's base arms keep the head: `0x5fdba0` re-zeroes the variation.
#[test]
fn engaged_base_arms_keep_the_head_variation() {
    let mut app = app();
    let (unit, nodes) = spawn_fidgeter(&mut app);
    app.world_mut().entity_mut(unit).insert(Engaged(0));
    app.update();
    let player = app.world().entity(unit).get::<AnimationPlayer>().unwrap();
    // No weapon: the Ready pick resolves down to Stand.
    assert!(player.animation(nodes[0]).is_some(), "the head variation");
    assert!(
        player.animation(nodes[1]).is_none(),
        "no roll while engaged"
    );
}

/// The sheath reconcile tests the requested id, so a model that plays Stand for the hold stows.
#[test]
fn cast_hold_stows_even_when_the_model_lacks_the_spell_anims() {
    let mut app = app();
    // A gnoll's shape: Stand and a Ready idle, no 51, and a baked lookup sending 51 to Stand.
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

/// A spell flinch is `0x60ea70(severity = 0)`: a decaying overlay, the base left untouched.
#[test]
fn spell_impact_wound_rides_the_secondary_slot() {
    let mut app = app();
    let model = ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),  // Stand
            clip(8, 2, false), // StandWound, the unengaged severity-0 pick
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

/// A spell flinch is severity 0, so `0x60ea70` picks CombatWound(9) engaged, StandWound(8) not.
#[test]
fn spell_flinch_picks_the_wound_by_engagement() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, 1, true),   // Stand
                clip(8, 2, false),  // StandWound
                clip(9, 3, false),  // CombatWound
                clip(10, 4, false), // CombatCritical, never a spell flinch
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

/// A CharProc-11 rate-override node refuses every flinch (`0x60eaac`–`0x60eac8`), by its presence:
/// kit 3071 at rate 1.0 closes the gate like a freeze at 0.0.
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

/// The whiff's 0.5× touches swings only, not a kit special in the same `Mode::Swing` slot.
#[test]
fn whiff_slowdown_spares_a_non_swing_oneshot() {
    let mut app = app();
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(57, 2, false), // Special1H, Eviscerate's kit anim
            clip(16, 3, false), // AttackUnarmed
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

    app.world_mut().write_message(EmoteAnim {
        entity: spinner,
        anim_id: 57,
        seq: 1,
    });
    app.world_mut().write_message(SwingMessage {
        attacker: swinger,
        victim: None,
        hit_info: 0,
        victim_state: 2, // dodge, a whiff
        damage: 0,
        displayed: true,
        seq: 2,
    });
    app.update();
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

/// The combat fast path (`0x5fe43c`–`0x5fe48b`): the first in wire order doubles, the second parks.
#[test]
fn same_frame_collision_fast_paths_the_second_combat_clip() {
    let mut app = app();
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(57, 2, false), // Special1H, Eviscerate's kit anim
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

    // spin_last: the swing arrives first on the wire.
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
    // swing_last: the spin arrives first.
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

/// The base recompute's `+0xd60` read plays a parked clip, at normal rate, once no one-shot is
/// live. The cache is set by hand: these stand-in clips never complete.
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

/// A movement-flag change re-arms the base over a full-body one-shot; steady flags let it finish.
#[test]
fn a_movement_flag_change_cuts_a_full_body_oneshot_immediately() {
    let mut app = app();
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(5, 2, true),   // Run
            clip(16, 3, false), // the bare-hands swing, 1.0 s
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

/// The cast pin tests `[9e8] & 0x20000f` (`0x5fde80`), never the turn bits.
#[test]
fn turning_in_place_never_unpins_the_stationary_cast_hold() {
    let mut app = app();
    let mut model = caster_model();
    model.clips.push(clip(11, 7, true)); // ShuffleLeft
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

/// JumpStart plays out over a swim re-latch; a ground landing still cuts it, stilled.
#[test]
fn the_swim_relatch_holds_the_kick_but_a_ground_cut_freezes_it() {
    let mut app = app();
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),   // Stand
            clip(41, 2, true),  // SwimIdle
            clip(42, 3, true),  // Swim
            clip(37, 4, false), // JumpStart, the kick
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

    // Swim re-latches mid-kick.
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

    // Landing on a bank instead.
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

/// A remote unit kneels on `UNIT_FLAG_LOOTING` (field 46); moving outranks it (`0x5fd8b0`).
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

    app.update();
    assert_eq!(gait(&app), Some(50), "looting kneels");

    app.world_mut().entity_mut(unit).insert(MovementState {
        speed: 7.0,
        flags: move_flags::FORWARD,
        ..Default::default()
    });
    app.update();
    assert_eq!(gait(&app), Some(5), "a moving looter runs");

    app.world_mut().entity_mut(unit).remove::<MovementState>();
    app.world_mut()
        .entity_mut(unit)
        .insert(crate::net::ObjectStore(ObjectFields::from_pairs(&[(
            46, 0,
        )])));
    app.update();
    assert_eq!(gait(&app), Some(0), "released — back to Stand");
}

/// The self unit kneels off its loot latch (`[player+0x1d28]`, `0x6126b0`), not the flag.
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
    // The descriptor flag is set, the latch empty.
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

    app.world_mut()
        .resource_mut::<crate::ui_loot::LootKneel>()
        .0 = true;
    app.update();
    assert_eq!(gait(&app), Some(50), "the armed latch kneels the self unit");

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

/// The CREEP vis flag (`UNIT_FIELD_BYTES_1` byte 3 bit 1, field 138) alone flips the prowl pose,
/// read off the self unit's own descriptor too.
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

    set_flag(&mut app, CREEP);
    app.update();
    assert_eq!(gait(&app), Some(120), "the CREEP bit crouches the idle");

    app.world_mut().entity_mut(unit).insert(MovementState {
        speed: 7.0,
        flags: move_flags::FORWARD,
        ..Default::default()
    });
    app.update();
    assert_eq!(gait(&app), Some(119), "the prowl outranks the speed tail");

    set_flag(&mut app, 0);
    app.update();
    assert_eq!(gait(&app), Some(5), "broken stealth runs again");
}

/// The watchdog (`0x719370`) re-arms a loop at each window's end through the variation walk.
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

/// A jumper: the airborne clips, Stand and Run, a kit cast with a masked variant, a combat pair.
fn jumper_model() -> ModelAnimations {
    let mut cast = clip(54, 7, false); // SpellCastOmni, a kit's release anim
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
            clip(57, 9, false),  // Special1H, a kit's combat one-shot
            clip(16, 10, false), // AttackUnarmed
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

/// A jump that relaunches inside a one-frame detachment, FALLING never dropping, is a jump.
#[test]
fn a_jump_out_of_a_micro_detachment_still_enters_the_jump_bracket() {
    let mut app = app();
    let unit = jumper(&mut app);
    app.update(); // settle: Stand
    let mode = |app: &App| app.world().entity(unit).get::<AnimDriver>().unwrap().mode;
    // A one-frame detachment: FALLING with a downward speed is a step-off arc.
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
    // The jump lands and relaunches within the frame, so FALLING never toggles.
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

/// Only a rise past the threshold launches, so a step-off keeps its freeze (`0x5fd8e8`).
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

/// A cast id is CLASS_A but not COMBAT, so a jump in place routes it full-body over the hang, and
/// with no plays mid-arc the airborne freeze holds it until an edge.
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

/// At touchdown the land pick (`0x602c60`) replaces a mid-air one-shot like any plain play.
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

/// Fall(40) plays once, on the substep FALLINGFAR latches (`0x61a820`), so a cast armed after it
/// holds bone 0 until the landing pick.
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

/// A moving jump's takeoff-frozen FORWARD bit routes a cast to the masked overlay.
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

/// A jump is a locomotion request, so a live full-body cast moves to the key-bone with its id, rate
/// and play position (`0x5fe919`) and bone 0 takes the jump.
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

/// With the key-bone armed, a locomotion request goes to bone 0 (`0x5fe912`), so a moving
/// caster's jump leaves the torso's hold running.
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

/// A finished key-bone one-shot is not stopped: op4 `param_3 = -1` holds its last frame in the
/// secondary slot and fades it to the base over 150 ms.
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
    // Small frames, so the 150 ms fade is still running when the clip completes.
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
    // Past 150 ms the slot self-releases, the kernel's `+0xd0 = -1`.
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

/// On a step-off arc no pin swaps the gait mid-air; the cast pin applies at touchdown.
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
    // A step-off, no jump arc, with vz ≠ 0: the `0x5fd8e8` freeze holds.
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: -3.0,
        ..Default::default()
    });
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
    app.world_mut()
        .entity_mut(unit)
        .insert(MovementState::default());
    app.update();
    assert_eq!(gait(&app), Some(51), "the pin lands with the unit");
}

/// The client clears the deferred cache only at plays, so a mid-air park lives until the land pick.
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
                  // A kit combat clip (57) replaces the bracket; a swing in the same batch parks.
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
    app.update();
    app.update();
    assert_eq!(
        drv(&app, unit).deferred,
        Some(16),
        "no play mid-arc — the park survives"
    );
    // Touchdown's land pick is a play, which clears the park (`0x5fe48e`).
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

/// The cache's consuming read (`0x5fd392`, in `0x5fd360`) sits below the airborne freeze, so a
/// mid-air park never plays mid-air, and the landing's play clears it.
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
    // Touchdown's play clears the cache unplayed (`0x5fe48e`).
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

/// A landed swing snaps its attacker to melee (`0x625829` in `0x6255b0`); the reconcile alone never
/// leaves the ranged stance, its melee force being gated on `CUR != 2` (`0x5fe0f9`, `0x5fe13b`).
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

    // The shot's draw: `SetSheatheState(2, SNAP)`.
    app.world_mut().write_message(SheathRequest {
        entity: unit,
        state: 2,
        ceremony: false,
    });
    app.update();
    assert_eq!(sheath(&app), Some(2), "the shot draws the bow");

    swing(&mut app);
    swing(&mut app);
    assert_eq!(
        sheath(&app),
        Some(2),
        "the reconcile alone never leaves the ranged stance — this is the bug's shape"
    );

    // The packet arm's snap, which `creature_anim::net::attacker_state` writes beside the swing.
    app.world_mut().write_message(SheathRequest {
        entity: unit,
        state: 1,
        ceremony: false,
    });
    swing(&mut app);
    assert_eq!(sheath(&app), Some(1), "the landed swing draws melee");

    swing(&mut app);
    assert_eq!(sheath(&app), Some(1), "melee holds across the volley");
}

/// The Z toggle stows both hands with the setter's play (`0x611b60`), and only then does the
/// finish drawer (`0x5fc920` at `0x5fca8c`, `0x5fcaa1`) reach for the bow.
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
    // Per-arm masked pairs; with no `$SHL`/`$SHR` a weapon moves at the halfway fallback.
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
    // Hip sword (3: HipSheath 90), back shield (4: Sheath 89), back bow (1: 89, and
    // INVTYPE_RANGED puts it on the left arm).
    let unit = app
        .world_mut()
        .spawn((
            anims,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
            // The Z toggle is the local player's alone.
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

    // Snap to melee-drawn, then press Z.
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

    // [2, 0], the right arm settled and the left still empty, exists only if a second clip
    // started after the stows finished.
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

/// A root that wipes the direction bits the frame a cast arrives: against the base's flags the edge
/// re-arms Stand(0), not locomotion, so Stand overwrites the cast with no transplant.
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

    // One frame: the root lands and the cast arrives.
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

/// The control: a locomotion re-arm still transplants the cast to the torso.
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

/// Stand and a Walk(4) at the 2.5 yd/s design speed of `ogremage.m2` and `humanmale.m2`.
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

/// A mount shaped like Horse.m2: it authors neither Sprint 143 nor JumpLandRun 187 and its baked
/// lookup sends both to Run(5); Tiger.m2 and Cat.m2 carry the same 187 row.
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

fn walk_rate(app: &App, unit: Entity) -> f32 {
    app.world()
        .entity(unit)
        .get::<AnimationPlayer>()
        .unwrap()
        .animation(AnimationNodeIndex::new(2))
        .expect("the walk node is playing")
        .speed()
}

/// A Gordok Ogre-Mage (scale 2.2) walking at vmangos' `speed_walk` 1.6 × 2.5 = 4.0 yd/s cycles at
/// 4.0 / (2.5 × 2.2) = 0.73×; the same unit at scale 1.0 reads 1.60×.
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

/// The divisor reads the mount model (`[unit+0xdc] ?: [unit+0xd8]`), scaled by the rider's
/// `OBJECT_FIELD_SCALE_X` times its own display column (`0x613ef0`): 1.5 under 2.0 divides by 3.0.
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
    // 4.0 / (2.5 · 1.5 · 2.0) = 0.533…, at the rider's speed.
    assert!(
        (walk_rate(&app, mount) - 0.533_33).abs() < 1e-3,
        "the mount divides by rider x mount scale: got {}",
        walk_rate(&app, mount)
    );
}

/// A mount's JumpLandRun 187 resolves to Run(5), a rate-scaled clip, so the landing runs at
/// `speed / moveSpeed` like the gait: the rate write covers every mode.
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

    // Touchdown, still holding forward.
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

/// The mount arm (`0x607a00`'s tail, `0x607b44`) is an ordinary primary play of 91 `Mount` on
/// bone 0, so it displaces the summon's full-body cast clip at once.
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
            clip(MOUNT, 2, true),            // Mount, the seat pose
            clip(SPELL_CAST_OMNI, 3, false), // the cast release, full body
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

    mount_field(&mut app, 2404);
    app.update();
    assert_eq!(
        playing(&app),
        Some(MOUNT),
        "B203: the mount arm displaces the cast clip — it does not wait for it to finish"
    );

    app.update();
    assert_eq!(playing(&app), Some(MOUNT));

    // Dismount, the watcher's other leg: `0x607ce0` arms Stand on the same bone.
    mount_field(&mut app, 0);
    app.update();
    assert_eq!(playing(&app), Some(0), "back on its own feet, standing");
}

/// The mount watcher's two op4 `0x7121a0` calls differ in one literal: the build cross-fades
/// (`0x607b35 push 0x1`), the teardown cuts (`0x607d1c push 0x0`), seen in the outgoing weight.
#[test]
fn the_dismount_cuts_the_saddle_pose_where_the_mount_up_blends_into_it() {
    use benilla_protocol::ObjectFields;

    const FIELD_MOUNTDISPLAYID: u16 = 133;
    const MOUNT: u16 = 91;
    let stand_node = AnimationNodeIndex::new(1);
    let mount_node = AnimationNodeIndex::new(2);

    let mut app = app();
    // A long blend, so a fade cannot pass for a cut.
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

    mount_field(&mut app, 2404);
    app.update();
    assert_eq!(drv_of(&app, unit).active_anim(), Some(MOUNT));
    assert!(
        weight(&app, stand_node) > 0.5,
        "the mount-up cross-fades: the outgoing pose is still weighted in"
    );

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
            clip(105, 3, false), // LoadBow, the pull
            clip(109, 4, true),  // HoldBow, the drawn hold
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    }
}

/// `$BWP` sets the nock latch and `$BWR` clears it, and only standing clips carry either tag, so
/// moving drops the latch; the ammo display cache survives.
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

/// The drawn idle enters on the local auto-repeat bit `0x200` alone (`0x5fd460`), never `0x400`.
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
            // Set by any ranged spell's visual, and not cleared at volley end.
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

/// A fire clip's completion reaches the dispatcher `0x5fc3f0` via its deferred site (`0x719370`,
/// `0x7074b0`): slot 22 recomputes to a re-pull, whose `$BWP` re-nocks, and a finished Load arms
/// the Hold (slots 11/12/15).
#[test]
fn a_mid_volley_fire_clip_re_pulls_and_the_pull_promotes_to_the_hold() {
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    // Stand-in spans; only the order of completions matters.
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

    // HumanMale authors all four; only 109 is a loop.
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

    // The extra frame: clips advance in `PostUpdate`, after the driver, which sees a completion
    // one frame late.
    advance(&mut app, 1000);
    app.update();
    assert_eq!(
        gait(&app),
        Some(109),
        "a finished LoadBow yields HoldBow — the drawn pose the shooter sits in between shots"
    );

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

    advance(&mut app, 1000);
    app.update();
    assert_eq!(
        mode(&app),
        super::super::select::Mode::Gait,
        "the fire clip's completion recomputes the base (slot 22's bare RecomputeBaseAnim(-1))"
    );
    // The recompute clears the gait and re-picks it the next frame.
    app.update();
    assert_eq!(
        gait(&app),
        Some(105),
        "…and the shooter RE-PULLS: the per-shot reload, whose $BWP re-nocks the arrow"
    );

    advance(&mut app, 1000);
    app.update();
    assert_eq!(
        gait(&app),
        Some(109),
        "fire → re-pull → hold, once per shot"
    );

    // The volley ends: the cancel's `RecomputeBaseAnim(-1)`.
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

/// Every shot of a volley arms AttackBow(46): it is not in the combat set (`0x5fcc10`), so the
/// fast path (`0x5fe43c`) never parks it, and the same-id dedup (`0x5fdba0`) finds it finished.
#[test]
fn every_shot_of_a_volley_re_arms_the_fire_clip_through_the_emote_lane() {
    use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
    use bevy::animation::AnimationClip;

    /// Stand-in spans; a fire clip only has to be far shorter than a bow's ~3 s between shots.
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

    // Unlike `archer_model`, this authors 46, as HumanMale.m2 does.
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

    // `SMSG_SPELL_START`'s ranged snap draws, and the shooter pulls.
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
        // `SMSG_SPELL_GO`'s cast kit, through the router.
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

        // The ~3 s to the next shot as one frame: the clip finishes in `PostUpdate`, after the
        // driver has run, so the driver has not yet seen the completion.
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

    // The volley ends: the cancel's `RecomputeBaseAnim(-1)`.
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

/// A vendor on real clip assets with HumanMale's timings: Stand 2.667 s (blend 0.5 s), the shuffles
/// 0.5 s (blend 0.25 s) with `replay = (0,0)`, as all 1130 shipped shuffle records carry, so a
/// window is one span. Nodes: Stand, ShuffleLeft, ShuffleRight.
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
    // Five yards east of the origin, facing +x: its back to a player north-west of it.
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

/// The facing producer and this driver together: the latch holds while the yaw the pump applies
/// exceeds 1e-5, and the shuffle ends at its own window, since `0x607ed0`'s tail cannot stop one.
#[test]
fn the_interaction_face_me_shuffles_its_feet_for_the_whole_turn() {
    let mut app = app();
    app.init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::ui_session::InteractNpc>();
    app.add_systems(
        Update,
        crate::net::drive_display_facing.before(drive_animations),
    );
    // Off the vendor's axis at a bearing of ~2.60 rad; exactly ±pi would sit on the yaw wrap.
    app.world_mut().spawn((
        crate::net::SelfPlayer,
        crate::net::ActiveMover,
        Transform::from_translation(benilla_assets::coords::wow_to_bevy([0.0, 3.0, 0.0])),
    ));
    let (npc, nodes) = spawn_vendor(&mut app);

    let gait = |app: &App| app.world().entity(npc).get::<AnimDriver>().unwrap().gait;
    let turning = |app: &App| app.world().entity(npc).contains::<crate::net::FacingStep>();
    let shuffle_weight = |app: &App| {
        let p = app.world().entity(npc).get::<AnimationPlayer>().unwrap();
        nodes[1..]
            .iter()
            .filter_map(|n| p.animation(*n).map(|a| a.weight()))
            .fold(0.0f32, f32::max)
    };
    // `ms` of 16 ms frames: the peak shuffle weight, and whether `want` held the gait slot.
    let run = |app: &mut App, ms: u64, want: u16| {
        let (mut peak, mut held) = (0.0f32, true);
        for _ in 0..(ms / 16) {
            advance(app, 16);
            peak = peak.max(shuffle_weight(app));
            held &= gait(app) == Some(want);
        }
        (peak, held)
    };

    let (peak, _) = run(&mut app, 128, 0);
    assert_eq!(gait(&app), Some(0), "no window, no turn");
    assert_eq!(peak, 0.0, "and no shuffle to be seen");

    // Open its window. 256 ms covers the ease (ten pumps, ~160 ms) and the 250 ms blend-in.
    app.world_mut()
        .resource_mut::<crate::ui_session::InteractNpc>()
        .0 = Some(npc);
    let (peak, held) = run(&mut app, 256, 11);
    assert!(held, "the whole turn is ShuffleLeft, got {:?}", gait(&app));
    assert!(
        peak > 0.95,
        "the shuffle blends the whole way in; the frozen-feet defect peaked at 0.32, got {peak}"
    );

    // The ease has settled; the shuffle holds to its 500 ms window.
    assert!(!turning(&app), "the ease has settled");
    let (_, held) = run(&mut app, 192, 11);
    assert!(
        held,
        "the shuffle is held past the settle to its own window, got {:?}",
        gait(&app)
    );

    // Released at the window: `0x5fc3f0`'s completion row recomputes, and the chain answers Stand.
    run(&mut app, 160, 0);
    assert_eq!(
        gait(&app),
        Some(0),
        "the completed window returns it to Stand"
    );
    // Let Stand's 500 ms blend finish: a turn mid-fade would stack three tracks here, where the
    // reference refuses to re-seed a blend short of half weight (`0x7125d4`).
    run(&mut app, 640, 0);

    // Closing it swings the vendor back to its wire facing, shuffling the other way.
    app.world_mut()
        .resource_mut::<crate::ui_session::InteractNpc>()
        .0 = None;
    let (peak, held) = run(&mut app, 256, 12);
    assert!(held, "the swing back is ShuffleRight, got {:?}", gait(&app));
    assert!(peak > 0.95, "and blends the whole way in: {peak}");
}

/// Disarm has no selector case: `GetWeapon(slot, 0)` hands the swing (`0x6246a0`) and the Ready
/// idle (`0x5fcdc0`) a null hand, and their unarmed legs do the rest.
#[test]
fn a_disarmed_attacker_swings_and_stands_unarmed() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, 1, true),    // Stand
                clip(17, 2, false),  // Attack1H, the sword swing
                clip(16, 3, false),  // AttackUnarmed, the fist
                clip(26, 4, true),   // Ready1H
                clip(25, 5, true),   // ReadyUnarmed
                clip(88, 6, false),  // AttackOffPierce, the off-hand dagger
                clip(117, 7, false), // AttackUnarmedOff, the off-hand fist
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }
    // Sword and dagger, engaged; only the disarm bit differs.
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

    // A fresh pair per hand: a second swing over a live one would park (`0x5fcc10`).
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

    // The main-hand swing, HitInfo bit 0x4 clear.
    let (armed_nodes, disarmed_nodes) = swings(0);
    assert!(
        armed_nodes.contains(&2),
        "the armed control swings Attack1H: {armed_nodes:?}"
    );
    assert!(
        disarmed_nodes.contains(&3) && !disarmed_nodes.contains(&2),
        "the disarmed attacker swings AttackUnarmed(16), never the sword's clip: {disarmed_nodes:?}"
    );

    // The off-hand swing (bit 0x4): the gate's first probe is the main hand, and a weapon there
    // cancels it (`0x5ec28d je 0x5ec2aa`), so a disarmed dual-wielder keeps its dagger.
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

/// With no main-hand weapon the disarm falls to the off hand: AttackUnarmedOff(117) (`0x6246a0`).
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
                main: None,          // nothing here to take the disarm
                off: Some((2, 0xf)), // so the dagger is what it hides
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

/// At play time (`0x5fe2f0`) Special1H(57) turns SpecialUnarmed(118) only with both hands empty.
#[test]
fn a_special_goes_unarmed_only_when_both_hands_are_empty() {
    let model = || ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(0, 1, true),
            clip(57, 2, false),  // Special1H, the kit's weapon spin
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

/// `DO_NOT_PLAY_WOUND_ANIM` (`type_flags & 0x8`) refuses every flinch inside the flinch itself
/// (`0x60ea9f`); the control carries the neighbouring bits `0x30` and must still flinch.
#[test]
fn a_no_wound_creature_takes_no_flinch() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, 1, true),  // Stand
                clip(8, 2, false), // StandWound
            ],
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }
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
    // Not yet queried: the null-record leg passes (`0x6125f0`).
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

/// The flag also refuses the parry (`0x60ec1f` in the parry pick `0x60ec00`), which the `$CPP`
/// ladder enters only on victimState 3, so a flagged creature still dodges.
#[test]
fn the_no_wound_flag_takes_the_parry_but_not_the_dodge() {
    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, 1, true),   // Stand
                clip(21, 2, false), // Parry1H, a 1H sword's parry
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
    const FLAGGED: u32 = 3870; // Stone Sleeper, which carries the bit in the world DB
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
                // A 1H sword: `defense_anim` sends class 2 subclass 7 to Parry1H(21).
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

/// Locomotion reaches the one entry point `0x5fe2f0` too (`0x602c60` → `0x5fd9e0` → `0x5fd8b0` →
/// `0x5fd100`), so a gait change raises the trail edge; Charge's kit has anim id −1 (`0x60f366 jl`)
/// and waits on it. With no per-frame recompute, a steady unit raises nothing.
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
    app.update(); // settle: Stand, itself a play
    app.update();
    assert!(!edge(&app), "a settled, motionless unit plays nothing");
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
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 57,
        seq: 1,
    });
    app.update();
    assert!(edge(&app), "and so does a one-shot");
}

/// The combat fast path returns at `0x5fe48b`, before the trail latch's only read at `0x5fe48e`,
/// so a pending trail arm (one slot, `0x60d835`) survives it: no anim edge rises.
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
                  // Special1H(57) is in the combat set (`0x5fcc10`).
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: 57,
        seq: 1,
    });
    app.update();
    assert!(edge(&app), "the first combat play arms normally");
    // A second combat request, AttackUnarmed(16), while the first runs.
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

/// The base-animation lock: a stun root's recompute asks for `Stand(0)` and `0x5fe2f0`'s guard on
/// `[unit+0xd58] & 0xc0000` refuses it, so a `Knockdown` (Lash, spell 6607) runs its 2000 ms.
mod base_anim_lock {
    use super::*;
    use crate::creature_anim::{BaseAnimRecompute, Mode};

    /// Nodes of Stand and the locking Knockdown; SpecialUnarmed, which takes no lock, is node 3.
    const STAND_NODE: u32 = 1;
    const KNOCKDOWN_NODE: u32 = 2;

    fn model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, STAND_NODE, true),        // Stand
                clip(121, KNOCKDOWN_NODE, false), // Knockdown, takes the lock
                clip(118, 3, false),              // SpecialUnarmed, takes none
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

    /// A base `Knockdown` survives the stun root's recompute: the `Stand` it asks for is refused.
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

    /// An ordinary one-shot takes no lock, so the same recompute ends it.
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

    /// The locking Knockdown and the airborne clips.
    fn airborne_model() -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: vec![
                clip(0, STAND_NODE, true),        // Stand
                clip(121, KNOCKDOWN_NODE, false), // Knockdown, takes the lock
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

    /// The landing freeze stills only the arc's own clip, never the locked `Knockdown`, which must
    /// finish to release the lock.
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

        // A jump while knocked down: every clip of the bracket is refused.
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
