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
