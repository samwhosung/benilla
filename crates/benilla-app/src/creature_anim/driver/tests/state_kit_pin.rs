//! The stationary state-kit pin (`0x5fde80`): a live aura state kit's id takes a still unit's plays.

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::AnimationClip;
use bevy::prelude::*;

use benilla_assets::{AnimClip, ModelAnimations};

use super::super::super::{
    move_flags, AnimDriver, BaseAnimRecompute, EmoteAnim, MovementState, StateKitPins,
};
use super::{advance, app, clip};

/// Peon Sleeping (17743): its state kit 906 names EmoteSleep (71).
const PEON_SLEEPING: u32 = 17743;
const EMOTE_SLEEP: u16 = 71;
const SLEEP: u16 = 100;
const STAND: u16 = 0;
const EMOTE_CHEER: u16 = 68;
const SPAN: f32 = 1.0;

/// `OrcMale.m2`'s shape: Stand, Sleep and Cheer, no EmoteSleep, and a baked lookup row sending 71
/// to Sleep (100) with play code 0, as the shipped model has it. Nodes: Stand, Sleep, Cheer.
fn spawn_orc(app: &mut App) -> (Entity, [AnimationNodeIndex; 3]) {
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let mut c = AnimationClip::default();
            c.set_duration(SPAN);
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
    let seq = |id: u16, node, looping| AnimClip {
        node,
        duration: SPAN,
        blend_time: 0.0,
        replay: (0, 0),
        ..clip(id, 0, looping)
    };
    let mut lookup: Vec<_> = (0..=SLEEP)
        .map(|id| benilla_formats::PlayableAnim {
            resolved_id: id,
            dir_flags: 0,
        })
        .collect();
    lookup[usize::from(EMOTE_SLEEP)].resolved_id = SLEEP;
    let anims = ModelAnimations {
        graph: graph_handle.clone(),
        clips: vec![
            seq(STAND, nodes[0], true),
            seq(SLEEP, nodes[1], true),
            seq(EMOTE_CHEER, nodes[2], false),
        ],
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
            anims,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph_handle),
            AnimDriver::default(),
            MovementState::default(),
        ))
        .id();
    (unit, [nodes[0], nodes[1], nodes[2]])
}

fn sleeping() -> StateKitPins {
    StateKitPins(vec![(PEON_SLEEPING, EMOTE_SLEEP)])
}

fn base(app: &App, unit: Entity) -> Option<u16> {
    app.world()
        .entity(unit)
        .get::<AnimDriver>()
        .unwrap()
        .playing()
        .0
}

fn main_node(app: &App, unit: Entity) -> Option<AnimationNodeIndex> {
    app.world()
        .entity(unit)
        .get::<AnimationTransitions>()
        .unwrap()
        .get_main_animation()
}

fn elapsed(app: &App, unit: Entity, node: AnimationNodeIndex) -> f32 {
    app.world()
        .entity(unit)
        .get::<AnimationPlayer>()
        .unwrap()
        .animation(node)
        .map_or(-1.0, |a| a.elapsed())
}

/// The Valley of Trials case: vmangos sends the peon stand state 0, no emote state and the
/// Peon Sleeping aura; the recompute's Stand becomes EmoteSleep, which the model plays as Sleep.
#[test]
fn a_stationary_sleeping_aura_lays_the_unit_down() {
    let mut app = app();
    let (unit, nodes) = spawn_orc(&mut app);
    app.update();
    assert_eq!(
        base(&app, unit),
        Some(STAND),
        "no aura: the base pick stands"
    );

    app.world_mut().entity_mut(unit).insert(sleeping());
    app.update();
    assert_eq!(
        base(&app, unit),
        Some(EMOTE_SLEEP),
        "the pin takes the request"
    );
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[1]),
        "played as the model's Sleep"
    );
}

/// The pin outranks the stand-state pose: a sleeper's own Sleep request is pinned too.
#[test]
fn the_pin_outranks_a_stand_state_pose() {
    let mut app = app();
    let (unit, nodes) = spawn_orc(&mut app);
    app.world_mut().entity_mut(unit).insert((
        sleeping(),
        MovementState {
            stand_state: 1, // sitting: SitGroundDown would be asked for
            ..Default::default()
        },
    ));
    app.update();
    assert_eq!(base(&app, unit), Some(EMOTE_SLEEP));
    assert_eq!(main_node(&app, unit), Some(nodes[1]));
}

/// `[+0x9e8] & 0x20000f`: walking leaves the pin and stopping takes it again; a turn keeps it.
#[test]
fn only_a_still_unit_is_pinned() {
    let mut app = app();
    let (unit, _) = spawn_orc(&mut app);
    app.world_mut().entity_mut(unit).insert(sleeping());
    app.update();

    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::TURN_LEFT,
        ..Default::default()
    });
    app.update();
    assert_eq!(
        base(&app, unit),
        Some(EMOTE_SLEEP),
        "turning in place keeps the pin"
    );

    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD,
        speed: 2.5,
        ..Default::default()
    });
    app.update();
    assert_ne!(
        base(&app, unit),
        Some(EMOTE_SLEEP),
        "a walking unit is not pinned"
    );

    app.world_mut()
        .entity_mut(unit)
        .insert(MovementState::default());
    app.update();
    assert_eq!(base(&app, unit), Some(EMOTE_SLEEP), "stopped: pinned again");
}

/// A one-shot request on a pinned unit plays the pin, which the same-id dedup drops.
#[test]
fn a_one_shot_on_a_pinned_unit_plays_nothing_new() {
    let mut app = app();
    let (unit, nodes) = spawn_orc(&mut app);
    app.world_mut().entity_mut(unit).insert(sleeping());
    app.update();
    advance(&mut app, 300);
    let before = elapsed(&app, unit, nodes[1]);

    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: EMOTE_CHEER,
        seq: 1,
        via_player: false,
    });
    advance(&mut app, 100);
    assert_eq!(
        base(&app, unit),
        Some(EMOTE_SLEEP),
        "the cheer became the pin"
    );
    assert_eq!(main_node(&app, unit), Some(nodes[1]));
    assert!(
        elapsed(&app, unit, nodes[1]) > before,
        "and the sleep was not restarted"
    );
}

/// A pin arriving over a live one-shot cuts it at the stage-2 recompute.
#[test]
fn a_new_pin_cuts_a_live_one_shot() {
    let mut app = app();
    let (unit, nodes) = spawn_orc(&mut app);
    app.update();
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: EMOTE_CHEER,
        seq: 1,
        via_player: false,
    });
    app.update();
    assert_eq!(main_node(&app, unit), Some(nodes[2]), "the cheer plays");

    app.world_mut().entity_mut(unit).insert(sleeping());
    app.world_mut().write_message(BaseAnimRecompute {
        entity: unit,
        anim_id: EMOTE_SLEEP,
    });
    app.update();
    assert_eq!(base(&app, unit), Some(EMOTE_SLEEP));
    assert_eq!(main_node(&app, unit), Some(nodes[1]));
}

/// `0x5fdb50` reads the requested id (`+0xf8`): a second add edge naming EmoteSleep matches the
/// pinned 71, though the model plays Sleep (100), so nothing restarts.
#[test]
fn the_recompute_compares_the_requested_id_not_the_stand_in() {
    let mut app = app();
    let (unit, nodes) = spawn_orc(&mut app);
    app.world_mut().entity_mut(unit).insert(sleeping());
    app.update();
    advance(&mut app, 400);
    let before = elapsed(&app, unit, nodes[1]);
    assert!(before > 0.3, "the sleep is running ({before})");

    app.world_mut().write_message(BaseAnimRecompute {
        entity: unit,
        anim_id: EMOTE_SLEEP,
    });
    advance(&mut app, 50);
    assert!(
        elapsed(&app, unit, nodes[1]) > before,
        "no mismatch, so no re-pick restarted the clip"
    );
}

/// The aura's removal stops the effect and re-picks nothing (`0x614150`): the pinned clip plays
/// out its window, whose end re-picks Stand.
#[test]
fn the_pinned_clip_outlives_its_aura_until_its_window_ends() {
    let mut app = app();
    let (unit, nodes) = spawn_orc(&mut app);
    app.world_mut().entity_mut(unit).insert(sleeping());
    app.update();
    advance(&mut app, 400);

    app.world_mut().entity_mut(unit).remove::<StateKitPins>();
    advance(&mut app, 100);
    assert_eq!(
        base(&app, unit),
        Some(EMOTE_SLEEP),
        "mid-window: still asleep"
    );
    assert_eq!(main_node(&app, unit), Some(nodes[1]));

    for _ in 0..8 {
        advance(&mut app, 100);
    }
    assert_eq!(
        base(&app, unit),
        Some(STAND),
        "the window's end stands the unit up"
    );
    assert_eq!(main_node(&app, unit), Some(nodes[0]));
}

/// One pass pins the mount and the body (`0x5fdeee`, `0x5fdf08`): a still rider plays the kit's
/// id over Mount (91), and its mount child plays it too, off the host's still test.
#[test]
fn a_still_rider_and_its_mount_both_take_the_pin() {
    use benilla_protocol::ObjectFields;
    const FIELD_MOUNTDISPLAYID: u16 = 133;

    let mut app = app();
    let (rider, nodes) = spawn_orc(&mut app);
    let (mount, mount_nodes) = spawn_orc(&mut app);
    app.world_mut().entity_mut(rider).insert((
        crate::net::ObjectStore(ObjectFields::from_pairs(&[(FIELD_MOUNTDISPLAYID, 2402)])),
        crate::entities::mount::MountChild(mount),
        sleeping(),
    ));
    app.world_mut()
        .entity_mut(mount)
        .remove::<MovementState>()
        .insert(crate::entities::mount::MountBody { host: rider });
    app.update();
    assert_eq!(
        base(&app, rider),
        Some(EMOTE_SLEEP),
        "the rider's Mount is pinned"
    );
    assert_eq!(main_node(&app, rider), Some(nodes[1]));
    assert_eq!(
        base(&app, mount),
        Some(EMOTE_SLEEP),
        "the mount takes its host's pin"
    );
    assert_eq!(main_node(&app, mount), Some(mount_nodes[1]));

    app.world_mut().entity_mut(rider).insert(MovementState {
        flags: move_flags::FORWARD,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    assert_ne!(
        base(&app, rider),
        Some(EMOTE_SLEEP),
        "a moving rider is not pinned"
    );
    assert_ne!(base(&app, mount), Some(EMOTE_SLEEP), "nor is its mount");
}

/// FALLING is outside `0x20000f`, and the jump's own play (`0x60e4d9`) reaches the pin: a still
/// unit's JumpStart becomes the kit's id.
#[test]
fn a_still_units_jump_plays_the_pin() {
    let mut app = app();
    let (unit, nodes) = spawn_orc(&mut app);
    app.world_mut().entity_mut(unit).insert(sleeping());
    app.update();
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FALLING,
        vertical_speed: 7.0,
        ..Default::default()
    });
    app.update();
    assert_eq!(
        base(&app, unit),
        Some(EMOTE_SLEEP),
        "no JumpStart over the pin"
    );
    assert_eq!(main_node(&app, unit), Some(nodes[1]));
}

const RUN: u16 = 5;
const ATTACK_UNARMED: u16 = 16;
const READY_2H_LOOSE: u16 = 28;
const READY_SPELL: u16 = 51;
const SPELL_CAST_DIRECTED: u16 = 53;

/// Stand, Run, a one-shot AttackUnarmed, the staff Ready idle, the spell-ready stance and a cast,
/// on real clip assets. Nodes in that order.
fn spawn_runner(app: &mut App) -> (Entity, [AnimationNodeIndex; 6]) {
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let mut c = AnimationClip::default();
            c.set_duration(SPAN);
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
    let seq = |id: u16, node, looping| AnimClip {
        node,
        duration: SPAN,
        blend_time: 0.0,
        replay: (0, 0),
        ..clip(id, 0, looping)
    };
    let anims = ModelAnimations {
        graph: graph_handle.clone(),
        clips: vec![
            seq(STAND, nodes[0], true),
            seq(RUN, nodes[1], true),
            seq(ATTACK_UNARMED, nodes[2], false),
            seq(READY_2H_LOOSE, nodes[3], true),
            seq(READY_SPELL, nodes[4], false),
            seq(SPELL_CAST_DIRECTED, nodes[5], false),
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
            MovementState::default(),
        ))
        .id();
    (
        unit,
        [nodes[0], nodes[1], nodes[2], nodes[3], nodes[4], nodes[5]],
    )
}

/// `0x5fd9e0(-1)` re-picks Run for a running unit, and `PlayTrack` drops the request for the id
/// the track already asks for (`0x5fdcab`): a Stand state kit landing on a runner restarts nothing.
#[test]
fn a_state_kit_recompute_leaves_a_running_units_clip_running() {
    let mut app = app();
    let (unit, nodes) = spawn_runner(&mut app);
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    advance(&mut app, 300);
    assert_eq!(base(&app, unit), Some(RUN));
    let before = elapsed(&app, unit, nodes[1]);
    assert!(before > 0.25, "the run is under way ({before})");

    app.world_mut().write_message(BaseAnimRecompute {
        entity: unit,
        anim_id: STAND,
    });
    advance(&mut app, 50);
    assert_eq!(base(&app, unit), Some(RUN));
    assert_eq!(main_node(&app, unit), Some(nodes[1]));
    assert!(
        elapsed(&app, unit, nodes[1]) > before,
        "the re-pick asked for Run again, so the clip kept its time"
    );
}

/// A Stand request on a standing unit asks for what the base already asks for and plays nothing.
#[test]
fn a_stand_request_leaves_an_idle_units_clip_running() {
    let mut app = app();
    let (unit, nodes) = spawn_runner(&mut app);
    app.update();
    advance(&mut app, 300);
    let before = elapsed(&app, unit, nodes[0]);
    assert!(before > 0.25, "the idle is under way ({before})");

    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: STAND,
        seq: 1,
        via_player: false,
    });
    advance(&mut app, 50);
    assert_eq!(main_node(&app, unit), Some(nodes[0]));
    assert!(
        elapsed(&app, unit, nodes[0]) > before,
        "the idle was not restarted"
    );
}

/// The demotion drop (`0x5feb00`-`0x5feb56`): with no key bone armed, a Stand request over a swing
/// is dropped, so a Stand impact kit does not cut a melee swing.
#[test]
fn a_stand_request_over_a_swing_is_dropped() {
    let mut app = app();
    let (unit, nodes) = spawn_runner(&mut app);
    app.update();
    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: ATTACK_UNARMED,
        seq: 1,
        via_player: false,
    });
    advance(&mut app, 100);
    assert_eq!(main_node(&app, unit), Some(nodes[2]), "the swing plays");
    let before = elapsed(&app, unit, nodes[2]);

    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: STAND,
        seq: 2,
        via_player: false,
    });
    advance(&mut app, 50);
    assert_eq!(main_node(&app, unit), Some(nodes[2]), "the swing is kept");
    assert!(elapsed(&app, unit, nodes[2]) > before, "and runs on");
}

/// The same drop over a ready stance: an engaged unit keeps its Ready idle through a Stand kit.
#[test]
fn a_stand_request_over_a_ready_stance_is_dropped() {
    let mut app = app();
    let (unit, nodes) = spawn_runner(&mut app);
    app.world_mut().entity_mut(unit).insert((
        crate::creature_anim::Engaged(0),
        crate::creature_anim::Wielded {
            main: Some((2, 0xa)), // a staff
            main_sheath: 2,
            ..Default::default()
        },
    ));
    app.update();
    advance(&mut app, 100);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[3]),
        "engaged: the ready idle"
    );
    let before = elapsed(&app, unit, nodes[3]);

    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: STAND,
        seq: 1,
        via_player: false,
    });
    advance(&mut app, 50);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[3]),
        "the ready stance is kept"
    );
    assert!(elapsed(&app, unit, nodes[3]) > before, "and not restarted");
}

/// The cast arm (`0x5feb26`-`0x5feb49`): over a cast or spell-ready stance Stand and a spell-ready
/// stance are dropped, and any other request plays.
#[test]
fn a_stand_request_over_a_cast_or_spell_ready_stance_is_dropped() {
    let mut app = app();
    let (unit, nodes) = spawn_runner(&mut app);
    app.update();
    let play = |app: &mut App, anim_id: u16| {
        app.world_mut().write_message(EmoteAnim {
            entity: unit,
            anim_id,
            seq: 1,
            via_player: false,
        });
        advance(app, 50);
    };
    play(&mut app, SPELL_CAST_DIRECTED);
    assert_eq!(main_node(&app, unit), Some(nodes[5]), "the cast plays");
    play(&mut app, STAND);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[5]),
        "Stand over a cast is dropped"
    );
    play(&mut app, READY_SPELL);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[5]),
        "so is a spell-ready stance"
    );
    play(&mut app, ATTACK_UNARMED);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[2]),
        "a swing over a cast plays"
    );

    play(&mut app, READY_SPELL);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[4]),
        "a spell-ready stance over a swing plays"
    );
    play(&mut app, STAND);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[4]),
        "Stand over a spell-ready stance is dropped"
    );
    play(&mut app, SPELL_CAST_DIRECTED);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[5]),
        "a cast over a spell-ready stance plays"
    );
}

/// Where Stand really plays, over a run, a looping Stand set on bone 0 ends with its window,
/// `span · R` (`0x712784`): the sequence-end callback sends id 0 to the default case of `0x5fc3f0`
/// (`0x5fc416 ja 0x5fc45a`), which re-picks the base (`0x5fd9e0(-1)`) back to Run.
#[test]
fn a_stand_request_over_a_run_ends_with_its_window() {
    let mut app = app();
    let (unit, nodes) = spawn_runner(&mut app);
    app.world_mut().entity_mut(unit).insert(MovementState {
        flags: move_flags::FORWARD,
        speed: 7.0,
        ..Default::default()
    });
    app.update();
    advance(&mut app, 100);
    assert_eq!(main_node(&app, unit), Some(nodes[1]), "running");

    app.world_mut().write_message(EmoteAnim {
        entity: unit,
        anim_id: STAND,
        seq: 1,
        via_player: false,
    });
    advance(&mut app, 100);
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[0]),
        "the kit's Stand plays"
    );

    for _ in 0..4 {
        advance(&mut app, 100);
    }
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[0]),
        "and holds for its window (one 1.0 s pass here)"
    );
    for _ in 0..8 {
        advance(&mut app, 100);
    }
    assert_eq!(
        main_node(&app, unit),
        Some(nodes[1]),
        "the window's end re-picks Run"
    );
}
