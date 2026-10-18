use super::*;
use benilla_ui::script::UiScript;
use benilla_ui::widget::{ANIM_SHUFFLE_LEFT, ANIM_SHUFFLE_RIGHT};

// ── The copy's key bone: the engine's arms, played ────────────────────────────────────────

/// The three ids a pane arms, with HumanMale's authored blend times (shuffles 0.25 s, Stand
/// 0.5 s). Two Stands, weighted so a roll never lands on the head, which the spawn plays.
fn turning_model() -> benilla_assets::ModelAnimations {
    let clip = |anim_id, node, blend_time, frequency| benilla_assets::AnimClip {
        anim_id,
        seq_index: 0,
        node: AnimationNodeIndex::new(node),
        // The blend slot's own track of each, nodes 5 to 8.
        blend_node: Some(AnimationNodeIndex::new(node + 4)),
        looping: true,
        duration: 0.5,
        move_speed: 0.0,
        blend_time,
        bounds_center: Vec3::ZERO,
        bounds_radius: 0.0,
        bounds_min: Vec3::ZERO,
        bounds_max: Vec3::ZERO,
        events: Vec::new().into(),
        arm_nodes: None,
        upper_node: None,
        frequency,
        replay: (0, 0),
        poses_bones: true,
    };
    benilla_assets::ModelAnimations {
        graph: Handle::default(),
        clips: vec![
            clip(ANIM_STAND, 1, 0.5, 1), // the head, all but unreachable by the roll
            clip(ANIM_STAND, 2, 0.5, 32766), // where a roll lands
            clip(ANIM_SHUFFLE_LEFT, 3, 0.25, 0), // the only variation
            clip(ANIM_SHUFFLE_RIGHT, 4, 0.25, 0),
        ],
        hand_close: [None, None],
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        first_seq: None,
        pose: Default::default(),
    }
}

fn node(n: usize) -> AnimationNodeIndex {
    AnimationNodeIndex::new(n)
}

fn weight(player: &AnimationPlayer, n: usize) -> Option<f32> {
    player.animation(node(n)).map(|a| a.weight())
}

fn arm(anim_id: u16, serial: u64, blend: bool) -> PaneArm {
    PaneArm {
        anim_id,
        serial,
        blend,
        offset_ms: 0,
    }
}

/// One copy's player and turn state, stepped at chosen wall-clock seconds.
struct Copy {
    anims: benilla_assets::ModelAnimations,
    catalog: benilla_formats::AnimDataCatalog,
    rng: benilla_assets::AnimRng,
    player: AnimationPlayer,
    turn: Turn,
}

impl Copy {
    /// A copy as [`build`] leaves it: the idle its load arms, Stand variation 0, and no arm
    /// played.
    fn spawned(anims: benilla_assets::ModelAnimations) -> Self {
        let mut player = AnimationPlayer::default();
        player.play(node(1)).repeat();
        Copy {
            anims,
            catalog: benilla_formats::AnimDataCatalog::from_rows([]),
            rng: benilla_assets::AnimRng::default(),
            player,
            turn: Turn {
                playing: Some(node(1)),
                ..Turn::default()
            },
        }
    }

    /// [`Self::spawned`] after the engine's first arm, its Stand (`0x5059a0`), at t = 0.
    fn standing() -> Self {
        let mut c = Copy::spawned(turning_model());
        c.step(arm(ANIM_STAND, 1, true), 0.0);
        c
    }

    fn step(&mut self, arm: PaneArm, now: f64) -> Option<u64> {
        step_copy(
            &mut self.player,
            &self.anims,
            &self.catalog,
            &mut self.rng,
            &mut self.turn,
            arm,
            now,
        )
    }
}

/// The copy's load arms its idle at once (`0x71d5d0` → `0x70ebd0`), so the Stand `0x505a2f`
/// arms, blend 1 with the latch clear, fades from it (`0x7125d6`) to a rolled variation.
#[test]
fn the_first_arm_on_a_fresh_copy_blends_from_its_idle() {
    let c = Copy::standing();
    assert_eq!(
        c.turn.playing,
        Some(node(2)),
        "a frequency roll, not the idle"
    );
    assert_eq!(
        c.turn.fade.map(|f| (f.node, f.until)),
        Some((node(1), 0.5)),
        "fading out of the idle over Stand's own 0.5 s"
    );
    assert_eq!(weight(&c.player, 2), Some(0.0), "the pose does not jump");
    assert_eq!(weight(&c.player, 1), Some(1.0));
}

/// Every arm `0x505bb0`/`0x505c50` makes carries `blendFlag = 1`, each over the incoming clip's
/// `blendTime`; the weights sum to 1 throughout.
#[test]
fn a_turn_blends_both_ways_over_the_incoming_clips_own_time() {
    let mut c = Copy::standing();
    // `SetRotation` arms the shuffle: it contributes nothing yet.
    c.step(arm(ANIM_SHUFFLE_LEFT, 2, true), 1.0);
    assert_eq!(c.turn.playing, Some(node(3)), "the shuffle is the primary");
    assert_eq!(weight(&c.player, 3), Some(0.0), "and it starts at nothing");
    assert_eq!(weight(&c.player, 2), Some(1.0), "Stand still holds");

    // Mid-blend, on the shuffle's 0.25 s; a held arrow arms nothing new.
    c.step(arm(ANIM_SHUFFLE_LEFT, 2, true), 1.2);
    let (out, inc) = (weight(&c.player, 2).unwrap(), weight(&c.player, 3).unwrap());
    assert!((out + inc - 1.0).abs() < 1e-5, "{out} + {inc}");
    assert!(inc > 0.8, "four fifths in, the shuffle dominates: {inc}");

    // Past 0.25 s the fade is done and the pose it faded out of is gone from the player.
    c.step(arm(ANIM_SHUFFLE_LEFT, 2, true), 1.3);
    assert_eq!(weight(&c.player, 3), Some(1.0));
    assert_eq!(weight(&c.player, 2), None, "the outgoing Stand is stopped");
    assert!(c.turn.fade.is_none());

    // The paint's return to Stand (`0x505c98`): this frame must not move the doll.
    c.step(arm(ANIM_STAND, 3, true), 1.41);
    assert_eq!(c.turn.playing, Some(node(2)), "a fresh roll");
    assert_eq!(
        weight(&c.player, 2),
        Some(0.0),
        "the release frame does not jump"
    );
    assert_eq!(
        weight(&c.player, 3),
        Some(1.0),
        "the shuffle still holds the pose"
    );

    // It runs for Stand's 0.5 s, so it is still blending 0.3 s in.
    c.step(arm(ANIM_STAND, 3, true), 1.71);
    let (out, inc) = (weight(&c.player, 3).unwrap(), weight(&c.player, 2).unwrap());
    assert!((out + inc - 1.0).abs() < 1e-5, "{out} + {inc}");
    assert!(
        out > 0.0 && inc > 0.0,
        "both still contribute: {out} / {inc}"
    );

    c.step(arm(ANIM_STAND, 3, true), 1.92);
    assert_eq!(weight(&c.player, 2), Some(1.0), "settled on Stand");
    assert_eq!(
        weight(&c.player, 3),
        None,
        "the shuffle is stopped, not muted"
    );
}

/// Inside a blend's first half `0x7125d4` refuses the arm; at or past halfway the arm takes the
/// one secondary slot.
#[test]
fn a_reversal_is_refused_inside_the_half_blend_and_displaces_it_after() {
    // Reverse at 0.1 s of a 0.25 s blend: 60% still to run, λ = 0.648, refused.
    let mut c = Copy::standing();
    c.step(arm(ANIM_SHUFFLE_LEFT, 2, true), 1.0);
    c.step(arm(ANIM_SHUFFLE_RIGHT, 3, true), 1.1);
    assert_eq!(
        c.turn.playing,
        Some(node(4)),
        "the opposite shuffle took over"
    );
    assert_eq!(
        c.turn.fade.map(|f| (f.node, f.until)),
        Some((node(2), 1.25)),
        "the ORIGINAL fade is untouched — same pose, same window"
    );
    assert_eq!(
        weight(&c.player, 3),
        None,
        "the first shuffle is dropped, not faded"
    );

    // Reverse at 0.2 s instead: 20% still to run, λ = 0.104, the slot is taken.
    let mut c = Copy::standing();
    c.step(arm(ANIM_SHUFFLE_LEFT, 2, true), 1.0);
    c.step(arm(ANIM_SHUFFLE_RIGHT, 3, true), 1.2);
    assert_eq!(
        c.turn.fade.map(|f| f.node),
        Some(node(3)),
        "…fading out of the first"
    );
    assert_eq!(weight(&c.player, 2), None, "the displaced Stand is gone");
}

/// Stand blends in over 0.5 s, so a nudge within 250 ms of a release meets a blend more than
/// half to run.
#[test]
fn a_second_nudge_inside_stands_own_blend_is_refused() {
    let mut c = Copy::standing();
    c.step(arm(ANIM_SHUFFLE_LEFT, 2, true), 1.0);
    // Released: the paint arms Stand, fading out of the shuffle over Stand's own 0.5 s.
    c.step(arm(ANIM_STAND, 3, true), 1.3);
    let settling = c.turn.playing.expect("Stand armed");
    assert_eq!(c.turn.fade.map(|f| (f.node, f.until)), Some((node(3), 1.8)));

    // Nudged again 100 ms later: 80% of Stand's blend still to run, λ = 0.896, refused.
    c.step(arm(ANIM_SHUFFLE_RIGHT, 4, true), 1.4);
    assert_eq!(
        c.turn.playing,
        Some(node(4)),
        "the new shuffle is the primary"
    );
    assert_eq!(
        c.turn.fade.map(|f| (f.node, f.until)),
        Some((node(3), 1.8)),
        "still fading the shuffle it was already fading, on the same window"
    );
    assert_eq!(
        c.player.animation(settling).map(|a| a.weight()),
        None,
        "the half-faded Stand is dropped, not layered on"
    );
}

/// `SetSequence` arms with blend 0 (`0x76cf60`), which clears the secondary slot (`0x712635`): a
/// cut, even over a fade in flight.
#[test]
fn a_set_sequence_arm_cuts_over_a_fade_in_flight() {
    let mut c = Copy::standing();
    c.step(arm(ANIM_SHUFFLE_LEFT, 2, true), 1.0);
    assert!(c.turn.fade.is_some());
    c.step(arm(ANIM_SHUFFLE_RIGHT, 3, false), 1.1);
    assert!(c.turn.fade.is_none(), "the secondary slot is cleared");
    assert_eq!(
        weight(&c.player, 4),
        Some(1.0),
        "the new pose lands at once"
    );
    assert_eq!(weight(&c.player, 2), None, "the fading Stand is stopped");
    assert_eq!(
        weight(&c.player, 3),
        None,
        "and so is the shuffle it replaced"
    );
}

/// The completion callback runs at the play window's end, `start + span·R` (`0x712692`), loops
/// included, once per arm; `SetSequenceTime`'s offset starts the window that far in.
#[test]
fn an_arm_reports_its_natural_end_once() {
    let mut c = Copy::standing();
    let stand = arm(ANIM_STAND, 1, true);
    assert_eq!(c.step(stand, 0.49), None, "the 0.5 s window still runs");
    assert_eq!(
        c.step(stand, 0.5),
        Some(1),
        "its end, for the arm that made it"
    );
    assert_eq!(c.step(stand, 0.6), None, "reported once");

    // The callback's Stand (`0x5062e4`), blend 1 with the latch set: the same sequence restarts
    // with no blend slot (`0x712553`).
    c.player.play(node(2)).seek_to(0.4);
    c.step(arm(ANIM_STAND, 2, true), 0.6);
    assert_eq!(c.turn.playing, Some(node(2)));
    assert!(c.turn.fade.is_none(), "no blend slot");
    assert_eq!(
        c.player.animation(node(2)).map(|a| a.seek_time()),
        Some(0.0),
        "restarted"
    );

    // Two plays drawn from the replay range: the window doubles.
    let mut anims = turning_model();
    for clip in &mut anims.clips {
        clip.replay = (2, 2);
    }
    let mut c = Copy::spawned(anims);
    c.step(stand, 0.0);
    assert_eq!(c.step(stand, 0.99), None);
    assert_eq!(c.step(stand, 1.0), Some(1));

    // 200 ms in, the window closes 200 ms early.
    let mut c = Copy::standing();
    let late = PaneArm {
        offset_ms: 200,
        ..arm(ANIM_SHUFFLE_LEFT, 2, false)
    };
    assert_eq!(c.step(late, 1.0), None);
    assert_eq!(c.step(late, 1.3), Some(2));
}

/// `0x7121a0` resolves an id the model lacks through its fallback (`0x711bf0`, default Stand),
/// so a pet with no shuffles plays Stand for one. That re-arm of the running Stand, latch clear,
/// copies it to the secondary on its own clock (`0x71255f` → `0x7125d6`) and restarts the primary
/// on the clip's other track; the paint's Stand return, inside that blend's first half, is refused
/// (`0x7125d4`) and restarts the primary alone while the idle fades on.
#[test]
fn a_display_with_no_shuffle_turns_on_stand_and_fades_from_the_idles_own_clock() {
    let mut anims = turning_model();
    anims.clips.retain(|c| c.anim_id == ANIM_STAND);
    // The model's own baked resolution: both shuffles fall back to Stand.
    anims.playable_animation_lookup = vec![
        benilla_formats::PlayableAnim {
            resolved_id: ANIM_STAND,
            dir_flags: 0,
        };
        usize::from(ANIM_SHUFFLE_RIGHT) + 1
    ];
    // A long idle, so no window closes inside the test.
    for clip in &mut anims.clips {
        clip.duration = 5.0;
    }
    let mut c = Copy::spawned(anims);
    c.step(arm(ANIM_STAND, 1, true), 0.0);
    c.step(arm(ANIM_STAND, 1, true), 0.6); // the idle's fade done
    c.player.play(node(2)).seek_to(0.3);
    c.step(arm(ANIM_SHUFFLE_LEFT, 2, true), 1.0);
    assert_eq!(
        c.turn.playing,
        Some(node(6)),
        "the substitute restarts on Stand's second track"
    );
    assert!(c.turn.ends.is_some(), "an arm that played owes its end");
    assert_eq!(c.turn.fade.map(|f| (f.node, f.until)), Some((node(2), 1.5)));
    // The paint's return, 110 ms on.
    c.step(arm(ANIM_STAND, 3, true), 1.11);
    assert_eq!(c.turn.playing, Some(node(6)));
    assert_eq!(
        c.turn.fade.map(|f| (f.node, f.until)),
        Some((node(2), 1.5)),
        "the refused arm keeps the window"
    );
    assert_eq!(
        c.player.animation(node(2)).map(|a| a.seek_time()),
        Some(0.3),
        "the idle fades out on its own clock, no snap to frame 0"
    );
    assert_eq!(
        c.player.animation(node(6)).map(|a| a.seek_time()),
        Some(0.0)
    );
    let (out, inc) = (weight(&c.player, 2).unwrap(), weight(&c.player, 6).unwrap());
    assert!((out + inc - 1.0).abs() < 1e-5 && out > 0.5, "{out} / {inc}");
}

/// A held arrow, frame by frame: the shuffle runs to its end, the callback arms Stand (`0x5062e4`),
/// which copies the shuffle to the secondary (`0x7125d6`), and the next `SetRotation` re-arms the
/// shuffle inside Stand's first half, refused (`0x7125d4`). The secondary keeps the old shuffle on
/// its clock while the new one runs from 0 on the clip's other track.
fn held_turn() -> Copy {
    let mut c = Copy::standing();
    let first = arm(ANIM_SHUFFLE_RIGHT, 2, true);
    c.step(first, 1.0);
    c.step(first, 1.3); // the shuffle's own fade done
    c.player.play(node(4)).seek_to(0.45);
    assert_eq!(c.step(first, 1.5), Some(2), "its window closes");
    c.step(arm(ANIM_STAND, 3, true), 1.517);
    c.step(arm(ANIM_SHUFFLE_RIGHT, 4, true), 1.533);
    c
}

#[test]
fn a_held_turns_rearm_inside_stands_blend_keeps_the_old_shuffle_on_its_own_clock() {
    let c = held_turn();
    let fade = c.turn.fade.expect("Stand's fade still runs");
    assert_eq!(fade.node, node(4), "fading the old shuffle");
    assert!(
        (fade.until - 2.017).abs() < 1e-9,
        "on Stand's window, untouched"
    );
    assert_eq!(
        c.player.animation(node(4)).map(|a| a.seek_time()),
        Some(0.45),
        "the old shuffle is not seeked back"
    );
    assert_eq!(c.turn.playing, Some(node(8)), "the new one is on the twin");
    assert_eq!(
        c.player.animation(node(8)).map(|a| a.seek_time()),
        Some(0.0)
    );
    let (out, inc) = (weight(&c.player, 4).unwrap(), weight(&c.player, 8).unwrap());
    assert!(
        out > 0.99 && (out + inc - 1.0).abs() < 1e-5,
        "{out} / {inc}"
    );
    assert_eq!(
        weight(&c.player, 2),
        None,
        "the overwritten Stand is dropped"
    );
}

/// At the window's end (`0x7147b1`) the secondary slot alone is emptied; the primary runs on.
#[test]
fn the_fade_window_ends_the_secondary_alone() {
    let mut c = held_turn();
    c.player.play(node(8)).seek_to(0.3);
    c.step(arm(ANIM_SHUFFLE_RIGHT, 4, true), 2.02);
    assert!(c.turn.fade.is_none());
    assert_eq!(weight(&c.player, 4), None, "the old shuffle is stopped");
    let primary = c
        .player
        .animation(node(8))
        .expect("the primary still plays");
    assert_eq!(
        (primary.seek_time(), primary.weight(), primary.repeat_mode()),
        (0.3, 1.0, RepeatAnimation::Forever),
        "on its clock, whole, still looping"
    );
}

/// The shortcut (`0x712553` → `0x71262d`) writes the primary alone: a fade in flight keeps its
/// slot, clock and window.
#[test]
fn a_latched_same_sequence_rearm_leaves_a_live_fade_alone() {
    let mut c = Copy::standing();
    // 400 ms in, the shuffle's window closes before its own 0.25 s fade from Stand does.
    let late = PaneArm {
        offset_ms: 400,
        ..arm(ANIM_SHUFFLE_LEFT, 2, true)
    };
    c.step(late, 1.0);
    c.player.play(node(2)).seek_to(0.2);
    assert_eq!(c.step(late, 1.1), Some(2));
    c.step(arm(ANIM_SHUFFLE_LEFT, 3, true), 1.15);
    assert_eq!(
        c.turn.fade.map(|f| (f.node, f.until)),
        Some((node(2), 1.25)),
        "the secondary slot is left as it is"
    );
    assert_eq!(
        c.player.animation(node(2)).map(|a| a.seek_time()),
        Some(0.2)
    );
    assert_eq!(c.turn.playing, Some(node(3)));
    assert_eq!(
        c.player.animation(node(3)).map(|a| a.seek_time()),
        Some(0.0),
        "restarted"
    );
    let (out, inc) = (weight(&c.player, 2).unwrap(), weight(&c.player, 3).unwrap());
    assert!(out > 0.0 && (out + inc - 1.0).abs() < 1e-5, "{out} / {inc}");
}

// ── The composite ─────────────────────────────────────────────────────────────────────────

/// A drawn pane's quad is its own booth's target at its own rect, rank, clip and own alpha
/// (`0x76d120`), premultiplied; a pane not drawn this frame composites nothing.
#[test]
fn a_drawn_pane_composites_its_own_target_at_its_own_rect() {
    let mut arena = benilla_ui::widget::WidgetArena::new();
    let h = arena.create(benilla_ui::widget::FrameKind::PlayerModel, None, None);
    let quiet = arena.create(benilla_ui::widget::FrameKind::PlayerModel, None, None);
    let mut images = Assets::<Image>::default();
    let (mut booths, mut state, mut requests) = (
        Booths::default(),
        PaneBooths::default(),
        PaneRequests::default(),
    );
    requests.reset(7);
    state.session = 7;
    for (h, x) in [(h, 0.0), (quiet, 300.0)] {
        let target = images.add(Image::default());
        let key = format!("pane:{h:?}");
        booths.0.insert(
            key.clone(),
            Booth::new(RenderLayers::layer(1), Entity::PLACEHOLDER, target),
        );
        state.panes.insert(
            h,
            PaneBooth {
                key,
                name: None,
                layer: 1,
                camera: Entity::PLACEHOLDER,
                size: UVec2::ONE,
                view: None,
                built: None,
                stand_in: None,
                turn: Turn::default(),
            },
        );
        requests.insert(
            h,
            PaneRequest {
                rect: Rect::new(x, 10.0, x + 233.0, 234.0),
                z_key: 42,
                alpha: 0.6,
                clip: Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
                size_px: UVec2::new(233, 224),
                units: Default::default(),
            },
        );
    }
    state.drawn.push(h);

    let quads = pane_quads(&state, &requests, &booths);
    assert_eq!(quads.len(), 1, "only the drawn pane");
    let q = &quads[0];
    assert_eq!(q.texture, state.target(h, &booths));
    assert_eq!(q.rect, Rect::new(0.0, 10.0, 233.0, 234.0));
    assert_eq!((q.z_key, q.color[3]), (42, 0.6));
    assert!(q.premultiplied);
    assert_eq!(q.clip, Some(Rect::new(0.0, 0.0, 100.0, 100.0)));

    // Another VM's requests never draw: its arena reissues the same handles.
    state.session = 8;
    assert!(pane_quads(&state, &requests, &booths).is_empty());
}

// ── The booth, end to end ─────────────────────────────────────────────────────────────────

const ME: u64 = 0x42;
const OTHER: u64 = 0x99;

/// The VM and the world a pane pass runs over: the player `ME` and a unit `OTHER`, each wearing
/// one mirrored part of its own mesh.
struct Harness {
    app: App,
    me_mesh: Handle<Mesh>,
    other_mesh: Handle<Mesh>,
    /// The unit ladder every pane's request carries.
    units: PaneUnits,
}

impl Harness {
    fn new(lua: &str) -> Self {
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            target: OTHER,
            ..Default::default()
        });
        script.run(lua).unwrap();
        let mut app = App::new();
        app.insert_non_send_resource(script)
            .init_resource::<PaneBooths>()
            .init_resource::<PaneRequests>()
            .init_resource::<PaneDisplays>()
            .init_resource::<Booths>()
            .init_resource::<BoothPanes>()
            .init_resource::<DressUpBake>()
            .init_resource::<GxAspect>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<WowModelMaterial>>()
            .init_resource::<benilla_world::rig_palette::RigPalettes>()
            .init_resource::<benilla_assets::AnimRng>()
            .init_resource::<Time<bevy::time::Real>>()
            .init_resource::<UiQuads>()
            .init_resource::<crate::ui_panes::CharacterPanes>()
            .init_resource::<crate::ui_panes::PaneAnswers>()
            .add_systems(
                Update,
                (
                    crate::ui_panes::feed_character_panes,
                    sync_panes,
                    compose_panes,
                )
                    .chain(),
            );
        let meshes = Assets::<Mesh>::default();
        let (me_mesh, other_mesh) = (meshes.reserve_handle(), meshes.reserve_handle());
        for (guid, mesh) in [(ME, me_mesh.clone()), (OTHER, other_mesh.clone())] {
            let unit = app.world_mut().spawn_empty().id();
            if guid == ME {
                app.world_mut().entity_mut(unit).insert(SelfPlayer);
            }
            app.world_mut().spawn((
                ChildOf(unit),
                super::super::PortraitPart {
                    static_mesh: mesh,
                    skinned_mesh: None,
                    material: Handle::default(),
                },
            ));
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(guid, unit);
        }
        Harness {
            app,
            me_mesh,
            other_mesh,
            units: PaneUnits::default(),
        }
    }

    fn script(&mut self) -> Mut<'_, UiScript> {
        self.app.world_mut().non_send_resource_mut::<UiScript>()
    }

    fn run(&mut self, lua: &str) {
        self.script().run(lua).unwrap();
    }

    /// The handle of the character pane named `name`, or of the anonymous one for `None`.
    fn handle(&mut self, name: Option<&str>) -> FrameHandle {
        self.script()
            .character_panes()
            .into_iter()
            .find(|p| p.name.as_deref() == name)
            .expect("the pane is listed")
            .handle
    }

    /// Two frames: the booth builds what the VM asks, and the feed hands the VM its answer, so
    /// the copy draws. Each publishes every listed pane's request, as the extract does.
    fn frame(&mut self) {
        for _ in 0..2 {
            self.pass();
        }
    }

    /// One frame of the pane pass.
    fn pass(&mut self) {
        // The append lane is the pass's to clear each frame (`ui_pass::clear_ui_overlays`).
        self.app
            .world_mut()
            .resource_mut::<UiQuads>()
            .overlays
            .clear();
        let script = self.app.world().non_send_resource::<UiScript>();
        let (session, panes) = (script.session(), script.character_panes());
        let units = self.units;
        let mut requests = self.app.world_mut().resource_mut::<PaneRequests>();
        requests.reset(session);
        for (i, p) in panes.iter().enumerate() {
            let x = 250.0 * i as f32;
            requests.insert(
                p.handle,
                PaneRequest {
                    rect: Rect::new(x, 0.0, x + 233.0, 224.0),
                    z_key: 9,
                    alpha: 1.0,
                    clip: None,
                    size_px: UVec2::new(233, 224),
                    units,
                },
            );
        }
        self.app.update();
    }

    /// The meshes standing in pane `h`'s booth; `None` with no booth.
    fn meshes(&mut self, h: FrameHandle) -> Option<Vec<Handle<Mesh>>> {
        let world = self.app.world_mut();
        let key = world.resource::<PaneBooths>().panes.get(&h)?.key.clone();
        let root = world.resource::<Booths>().0.get(&key)?.root;
        let mut q = world.query::<(&ChildOf, &Mesh3d)>();
        Some(
            q.iter(world)
                .filter(|(p, _)| p.parent() == root)
                .map(|(_, m)| m.0.clone())
                .collect(),
        )
    }

    /// The textures this frame's composite drew.
    fn drawn(&self) -> Vec<Option<Handle<Image>>> {
        self.app
            .world()
            .resource::<UiQuads>()
            .overlays
            .iter()
            .map(|q| q.texture.clone())
            .collect()
    }

    fn target(&self, h: FrameHandle) -> Option<Handle<Image>> {
        let world = self.app.world();
        world
            .resource::<PaneBooths>()
            .target(h, world.resource::<Booths>())
    }
}

/// `CharacterModelFrame:SetUnit("player")` (`PaperDollFrame.lua:39`) puts the player's copy in the
/// pane's own booth, which the composite draws; the engine holds the copy the booth built.
#[test]
fn set_unit_puts_the_units_copy_in_the_panes_own_booth() {
    let mut h = Harness::new(
        r#"m = CreateFrame("PlayerModel", "CharacterModelFrame", UIParent) m:SetUnit("player")"#,
    );
    let pane = h.handle(Some("CharacterModelFrame"));
    h.frame();
    assert_eq!(
        h.meshes(pane),
        Some(vec![h.me_mesh.clone()]),
        "the player's look"
    );
    let target = h.target(pane).expect("a booth of its own");
    assert_eq!(h.drawn(), vec![Some(target)], "its target, composited");
    let copy = h.script().character_pane(pane).unwrap().copy;
    assert!(
        copy.is_some_and(|c| c.build == 1),
        "the engine holds the copy: {copy:?}"
    );
    let marked = h.app.world().resource::<BoothPanes>().0.len();
    assert_eq!(marked, 1, "its camera renders while drawn");
}

/// The pane draws what its own `SetUnit` named, whatever its name: two panes, two subjects, two
/// booths; an addon's anonymous pane draws like a stock one.
#[test]
fn each_pane_draws_its_own_subject_named_or_not() {
    let mut h = Harness::new(
        r#"
        a = CreateFrame("PlayerModel", "InspectModelFrame", UIParent) a:SetUnit("target")
        b = CreateFrame("PlayerModel", nil, UIParent) b:SetUnit("player")
        "#,
    );
    let (inspect, anon) = (h.handle(Some("InspectModelFrame")), h.handle(None));
    h.frame();
    assert_eq!(h.meshes(inspect), Some(vec![h.other_mesh.clone()]));
    assert_eq!(h.meshes(anon), Some(vec![h.me_mesh.clone()]));
    assert_ne!(h.target(inspect), h.target(anon), "a target each");
    assert_eq!(h.drawn().len(), 2);
}

/// The camera spawns through `Commands` with the pane, and the pass that builds the copy aims it
/// with that spawn, so the frame the pane opens on already composites an aimed view, not the
/// spawn pose.
#[test]
fn a_pane_is_aimed_on_the_frame_it_opens() {
    let mut h =
        Harness::new(r#"m = CreateFrame("PlayerModel", "Doll", UIParent) m:SetUnit("player")"#);
    let pane = h.handle(Some("Doll"));
    h.pass();
    let world = h.app.world_mut();
    let booth = &world.resource::<PaneBooths>().panes[&pane];
    let (entity, aimed) = (booth.camera, booth.view.is_some());
    assert!(aimed, "the opening pass records the aim");
    assert!(
        matches!(world.get::<Projection>(entity), Some(Projection::Custom(_))),
        "the camera wears the body frame on the opening frame, not its spawn pose"
    );
}

/// The leg pane `h`'s booth camera draws through: orthographic or not.
fn ortho_leg(h: &mut Harness, pane: FrameHandle) -> bool {
    let world = h.app.world_mut();
    let camera = world.resource::<PaneBooths>().panes[&pane].camera;
    match world.get::<Projection>(camera) {
        Some(Projection::Orthographic(_)) => true,
        Some(Projection::Custom(_)) => false,
        other => panic!("not an aimed camera: {other:?}"),
    }
}

/// `SetCamera(n)` after `SetUnit` installs raw camera `n` on the copy (`0x76cec0`); past the
/// copy's table (this one has none) there is no camera, so the booth takes the orthographic leg
/// (`0x76d5f0`). `RefreshUnit`'s build installs the pane's own camera again (`0x505890`).
#[test]
fn set_camera_aims_the_booth_through_the_copys_camera_until_the_next_build() {
    let mut h = Harness::new(
        r#"m = CreateFrame("PlayerModel", "Portrait3D", UIParent) m:SetUnit("player") m:SetCamera(99)"#,
    );
    let pane = h.handle(Some("Portrait3D"));
    h.frame();
    assert!(
        ortho_leg(&mut h, pane),
        "no camera 99: the orthographic leg"
    );

    h.run("m:RefreshUnit()");
    h.frame();
    assert!(!ortho_leg(&mut h, pane), "the build's own camera");

    h.run("m:SetCamera(0)");
    h.frame();
    assert!(ortho_leg(&mut h, pane), "on the copy that stands, at once");
}

/// A particle's half-extent is in eye space (`0x7b2a50`): the orthographic leg's is the pane's
/// pixels per model unit, a perspective leg's is 1.0, and an emitter on the pane's layer follows
/// the leg the pane draws through, whenever it was spawned.
#[test]
fn a_panes_emitters_take_the_half_extent_unit_of_the_leg_it_draws_through() {
    let mut h = Harness::new(
        r#"m = CreateFrame("PlayerModel", "Portrait3D", UIParent) m:SetUnit("player")"#,
    );
    h.units.star_px_per_unit = 2000.0;
    let pane = h.handle(Some("Portrait3D"));
    h.frame();
    let layer = h.app.world().resource::<PaneBooths>().panes[&pane].layer;
    let em = benilla_assets::ModelEmitter {
        def: benilla_world::testing::plain_particle_def(),
        texture: Some(Handle::default()),
        bone_pivot: [0.0; 3],
        billboard: None,
        recursion: None,
        geometry: None,
        owner_reach: 0.0,
        water_bound: (Vec3::ZERO, 0.0),
        idle_seq: 0,
    };
    let world = h.app.world_mut();
    let e = {
        let mut commands = world.commands();
        let e = benilla_world::particles::spawn_emitter(
            &mut commands,
            &em,
            Transform::IDENTITY,
            benilla_world::particles::EmitterFrames {
                owner: None,
                anchor: None,
                alpha: None,
                light_node: None,
                on_owner_loss: benilla_world::particles::OwnerLoss::Free,
            },
            benilla_world::particles::EmitClock::Pinned,
        )
        .expect("an emitter");
        commands.entity(e).insert(RenderLayers::layer(layer));
        e
    };
    world.flush();
    let star = |h: &mut Harness| {
        h.app
            .world()
            .get::<benilla_world::particles::ParticleEmitter>(e)
            .unwrap()
            .size_scale()
    };
    h.frame();
    assert_eq!(star(&mut h), 1.0, "the pane's own camera is a perspective");

    h.run("m:SetCamera(99)");
    h.frame();
    assert!(ortho_leg(&mut h, pane));
    assert_eq!(star(&mut h), 2000.0, "the orthographic leg's px per unit");

    h.run("m:RefreshUnit()");
    h.frame();
    assert!(!ortho_leg(&mut h, pane));
    assert_eq!(star(&mut h), 1.0, "and back under a perspective");
}

/// The hide hook drops the copy (`0x505ce0`) and the booth with it; the show hook builds again
/// from the stored GUID (`0x505d3c`).
#[test]
fn hiding_releases_the_booth_and_showing_builds_again() {
    let mut h =
        Harness::new(r#"m = CreateFrame("PlayerModel", "Doll", UIParent) m:SetUnit("player")"#);
    let pane = h.handle(Some("Doll"));
    h.frame();
    assert!(h.meshes(pane).is_some());
    let target = h.target(pane);

    h.run("m:Hide()");
    h.frame();
    assert_eq!(h.meshes(pane), None, "no booth for a hidden pane");
    assert!(h.drawn().is_empty());
    assert!(h.app.world().resource::<Booths>().0.is_empty());
    assert!(h.app.world().resource::<BoothPanes>().0.is_empty());

    h.run("m:Show()");
    h.frame();
    assert_eq!(h.meshes(pane), Some(vec![h.me_mesh.clone()]));
    let copy = h.script().character_pane(pane).unwrap().copy;
    assert!(copy.is_some_and(|c| c.build == 2), "a new copy: {copy:?}");
    assert_eq!(
        h.target(pane),
        target,
        "drawn into the same target, not a new one"
    );
}

/// A pane past the target cap keeps its shape.
#[test]
fn an_oversized_pane_target_scales_down_whole() {
    assert_eq!(target_size(UVec2::new(233, 224)), UVec2::new(233, 224));
    assert_eq!(target_size(UVec2::new(8192, 2048)), UVec2::new(4096, 1024));
}

/// `RefreshUnit` (`0x505e40`) rebuilds from the GUID `SetUnit` stored, not from what the token
/// names now.
#[test]
fn refresh_unit_rebuilds_the_stored_guid() {
    let mut h =
        Harness::new(r#"m = CreateFrame("PlayerModel", "Doll", UIParent) m:SetUnit("target")"#);
    let pane = h.handle(Some("Doll"));
    h.frame();
    h.script().set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        target: ME,
        ..Default::default()
    });
    h.run("m:RefreshUnit()");
    h.frame();
    assert_eq!(
        h.meshes(pane),
        Some(vec![h.other_mesh.clone()]),
        "still the stored unit"
    );
    let copy = h.script().character_pane(pane).unwrap().copy;
    assert!(copy.is_some_and(|c| c.build == 2), "a fresh copy: {copy:?}");

    // `SetUnit` again resolves the token anew.
    h.run("m:SetUnit('target')");
    h.frame();
    assert_eq!(h.meshes(pane), Some(vec![h.me_mesh.clone()]));
}

/// A GUID the client holds no unit for builds nothing (`0x505b8d`) and the old copy stays; the
/// unit appearing later builds nothing either, until the next request.
#[test]
fn a_unit_the_client_does_not_hold_leaves_the_old_copy() {
    let mut h =
        Harness::new(r#"m = CreateFrame("PlayerModel", "Doll", UIParent) m:SetUnit("player")"#);
    let pane = h.handle(Some("Doll"));
    h.frame();
    h.script().set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        target: 0x777,
        ..Default::default()
    });
    h.run("m:SetUnit('target')");
    h.frame();
    assert_eq!(
        h.meshes(pane),
        Some(vec![h.me_mesh.clone()]),
        "the old copy"
    );
    assert_eq!(h.drawn().len(), 1, "still drawn");
    let c = h.script().character_pane(pane).unwrap();
    assert!(
        c.copy.is_some_and(|c| c.build == 1),
        "the engine's old copy: {c:?}"
    );
    assert!(!c.pending(), "the request is spent");

    // The unit streams in: nothing builds on its own.
    let other = h.app.world().resource::<crate::net::GuidIndex>().0[&OTHER];
    h.app
        .world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(0x777, other);
    h.frame();
    assert_eq!(
        h.meshes(pane),
        Some(vec![h.me_mesh.clone()]),
        "still the old copy"
    );
    // The next request does.
    h.run("m:RefreshUnit()");
    h.frame();
    assert_eq!(h.meshes(pane), Some(vec![h.other_mesh.clone()]));
}

/// A `SetModel` file takes `+0x318` from the copy; the booth goes, and a later `SetUnit` builds
/// the copy that replaces the file.
#[test]
fn a_file_replaces_the_copy_and_a_later_set_unit_replaces_the_file() {
    let mut h =
        Harness::new(r#"m = CreateFrame("PlayerModel", "Doll", UIParent) m:SetUnit("player")"#);
    let pane = h.handle(Some("Doll"));
    h.frame();
    h.run(r#"m:SetModel("Creature\\Wolf\\Wolf.mdx")"#);
    h.frame();
    assert_eq!(
        h.meshes(pane),
        None,
        "the file draws as a tile, not in a booth"
    );
    assert!(h.drawn().is_empty());

    h.run("m:SetUnit('target')");
    h.frame();
    assert_eq!(h.meshes(pane), Some(vec![h.other_mesh.clone()]));
    let c = h.script().character_pane(pane).unwrap();
    assert!(c.copy.is_some(), "the copy took the slot");
    assert_eq!(h.drawn().len(), 1);
}

/// The dressing room on the player (`DressUpFrame.lua:5`) wears the room's assembly, the
/// try-ons; another unit in a `DressUpModel` is mirrored as any pane's.
#[test]
fn the_dressing_room_on_the_player_wears_the_rooms_assembly() {
    let mut h = Harness::new(
        r#"
        m = CreateFrame("DressUpModel", "DressUpModel", UIParent) m:SetUnit("player")
        o = CreateFrame("DressUpModel", "Other", UIParent) o:SetUnit("target")
        "#,
    );
    let (room, other) = (h.handle(Some("DressUpModel")), h.handle(Some("Other")));
    let meshes = Assets::<Mesh>::default();
    let (tried_on, again) = (meshes.reserve_handle(), meshes.reserve_handle());
    {
        let mut bake = h.app.world_mut().resource_mut::<DressUpBake>();
        bake.parts = vec![super::super::PreviewPart {
            static_mesh: tried_on.clone(),
            skinned_mesh: None,
            material: Handle::default(),
            alpha_anim: None,
            twins: Default::default(),
        }];
        bake.revision = 1;
    }
    h.frame();
    assert_eq!(
        h.meshes(room),
        Some(vec![tried_on]),
        "the assembly, not the player"
    );
    assert_eq!(h.meshes(other), Some(vec![h.other_mesh.clone()]));

    // A new assembly re-dresses the copy that stands.
    {
        let mut bake = h.app.world_mut().resource_mut::<DressUpBake>();
        bake.parts[0].static_mesh = again.clone();
        bake.revision = 2;
    }
    h.frame();
    assert_eq!(h.meshes(room), Some(vec![again]));
}
