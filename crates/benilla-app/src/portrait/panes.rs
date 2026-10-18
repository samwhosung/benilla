//! The character model panes: each visible `PlayerModel`, `DressUpModel` and `TabardModel` draws
//! its own copy of the subject it stored, in a booth of its own.
//!
//! The copy duplicates a unit's model (`0x5059a0` → `0x707400`) or a creature display (`0x505a70`).
//! The engine keeps the pane's state ([`benilla_ui::widget::CharacterPane`]) and the feed lists the
//! panes ([`crate::ui_panes`]); [`sync_panes`] builds each copy, plays its arms and answers.
//! The camera is the one the copy holds (`0x505b30` → `0x505890`, `SetCamera`,
//! [`super::framing::body_view`]), the light its `CGLight` and fog
//! ([`crate::ui_models::PaneLights`]), the target its size in device pixels.

use std::collections::HashMap;

use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::RepeatAnimation;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{PerspectiveProjection, Projection, RenderTarget};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_ui::script::CharacterPaneFrame;
use benilla_ui::widget::{FrameHandle, FrameKind, PaneArm, ANIM_STAND};

use super::booth::{
    spawn_booth_effects, spawn_booth_model, BoothBillboardSpec, BoothEffects, BoothInstance,
    BoothMotion, BoothPart, BoothRider, BoothTwins,
};
use super::framing::{body_view, BodyView, PaneUnits, PortraitAnchors, PORTRAIT_FOV};
use super::{
    booth_anchors, hand_grip, log_bake, new_target_image_sized, wake_booth, Booth, BoothCam,
    BoothPanes, Booths, DressUpBake, DressedLook, GxAspect, PortraitStandIn, SnapKey,
    BOOTH_SETTLE_FRAMES,
};
use crate::entities::Creatures;
use crate::net::{NetEntity, SelfPlayer};
use crate::ui_models::{PaneLights, PaneScene};
use crate::ui_pass::{UiQuad, UiQuads};

/// The first pane booth layer. The pane block tops the booth ladder ([`super::GLUE_LAYER`]) and
/// grows with the panes on screen: a layer is a camera's identity, for rendering and for the
/// emitter-to-camera match.
pub(crate) const PANE_LAYER_BASE: usize =
    super::UI_MODEL_CAM_LAYER_BASE + super::UI_MODEL_CAM_LAYERS;

/// Every pane booth camera's order: before the tile cameras and the UI pass, which sample them.
/// Each renders its own target, so the shared order is no ambiguity.
const PANE_CAMERA_ORDER: isize = -50;

/// A pane target's largest edge, in device pixels.
const PANE_TARGET_MAX: u32 = 4096;

/// One pane's composite, published by the extract's `ModelPane` arm. An entry can outlive its
/// pane until the next full conversion, as a splice never says one vanished; the engine's paint
/// list decides what draws.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneRequest {
    /// The pane's rect in the quad pass's space, y-down logical px.
    pub(crate) rect: Rect,
    /// The pane's paint rank, its own bucket's ARTWORK callback.
    pub(crate) z_key: u128,
    /// The frame's own alpha, which the copy draws at (`0x76d120`).
    pub(crate) alpha: f32,
    /// The enclosing ScrollFrame's clip.
    pub(crate) clip: Option<Rect>,
    /// The rect in whole device pixels: the target's size.
    pub(crate) size_px: UVec2,
    /// The model-unit ladder the live and orthographic legs place the root by.
    pub(crate) units: PaneUnits,
}

/// The extract-to-booth bridge for the character panes, and the VM whose handles key it: a new
/// VM's arena reissues the same handles.
#[derive(Resource, Default)]
pub(crate) struct PaneRequests {
    map: HashMap<FrameHandle, PaneRequest>,
    session: u64,
}

impl PaneRequests {
    /// Forget every request, and key what follows to VM `session`.
    pub(crate) fn reset(&mut self, session: u64) {
        self.map.clear();
        self.session = session;
    }

    /// Publish pane `h`'s composite for this frame.
    pub(crate) fn insert(&mut self, h: FrameHandle, req: PaneRequest) {
        self.map.insert(h, req);
    }

    /// Pane `h`'s request, whichever VM published it.
    #[cfg(test)]
    pub(crate) fn of(&self, h: FrameHandle) -> Option<&PaneRequest> {
        self.map.get(&h)
    }

    /// Pane `h`'s request, if VM `session` published it.
    fn get(&self, session: u64, h: FrameHandle) -> Option<&PaneRequest> {
        (self.session == session)
            .then(|| self.map.get(&h))
            .flatten()
    }
}

/// The creature displays the stand-ins need, which the display cache loads with no wire entity
/// (a stable pane's pet, `0x505a70`).
#[derive(Resource, Default)]
pub(crate) struct PaneDisplays(pub(crate) Vec<u32>);

/// Every live pane booth, by frame handle, for one VM.
#[derive(Resource, Default)]
pub(crate) struct PaneBooths {
    session: u64,
    panes: HashMap<FrameHandle, PaneBooth>,
    /// Released layers, reused before the block grows.
    free_layers: Vec<usize>,
    next_layer: usize,
    /// Each hidden pane's last target and its size, so a re-show draws into it again.
    kept: HashMap<FrameHandle, (UVec2, Handle<Image>)>,
    /// The panes composited this frame, in the engine's registry order.
    drawn: Vec<FrameHandle>,
}

/// One pane's booth: its camera and target, and the copy it holds.
struct PaneBooth {
    /// Its key in [`Booths`] and its camera's [`BoothCam`] token.
    key: String,
    /// The frame's global name, for the instruments.
    name: Option<String>,
    layer: usize,
    camera: Entity,
    /// The target's size in device pixels.
    size: UVec2,
    /// The root's pose and the camera's aim, as last applied.
    view: Option<BodyView>,
    /// The copy standing in the booth.
    built: Option<Built>,
    /// The display stand-in a display subject is mirrored from.
    stand_in: Option<StandIn>,
    turn: Turn,
}

/// What a booth's copy was built from.
struct Built {
    /// The engine request it answers.
    build: u64,
    look: Look,
    /// The light scene its twins bind.
    scene: Option<PaneScene>,
    anchors: PortraitAnchors,
}

/// A copy's subject: a unit's GUID or a creature display.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Subject {
    Unit(u64),
    Display(u32),
}

/// What the copy wears, to see a re-dress.
#[derive(PartialEq)]
enum Look {
    /// A mirror of a live unit or a stand-in: the snapshot key.
    Mirror(SnapKey),
    /// The dressing room's assembly, by revision.
    Assembled(u64),
}

/// A display subject's stand-in entity ([`PortraitStandIn`]), built from the display cache.
struct StandIn {
    display: u32,
    entity: Entity,
    built: bool,
}

/// What the copy's key bone plays, as the booth runs the engine's arms on its player.
#[derive(Default)]
pub(super) struct Turn {
    /// The serial of the engine arm last played.
    played: Option<u64>,
    /// The node the primary runs, either of its clip's two tracks; the spawn's idle Stand until
    /// the first arm (`0x70ebd0`).
    pub(super) playing: Option<AnimationNodeIndex>,
    pub(super) fade: Option<Fade>,
    /// When the primary's window closes, `start + span·R` (`0x712692`); `None` once reported.
    ends: Option<f64>,
    /// The completion latch `[+0xc0]`: the primary ran to its end and called back.
    completed: bool,
}

/// The secondary blend slot (`[blk+0xc4]`, `0x7125d6`): the pose an arm replaced, on its own clock
/// and a track the primary is not using, faded out by λ.
#[derive(Clone, Copy)]
pub(super) struct Fade {
    pub(super) node: AnimationNodeIndex,
    /// When λ reaches 0 (`[blk+0x100]`).
    pub(super) until: f64,
    /// The incoming clip's `M2Sequence.blendTime` (`0x7125f2`).
    span: f32,
}

/// The fade still to run, 1 to 0: `(blendEnd − now) · blendRate`, recomputed every frame.
fn fade_frac(fade: &Fade, now: f64) -> f32 {
    ((fade.until - now) / f64::from(fade.span)) as f32
}

/// Empty the secondary slot, stopping its track.
fn clear_fade(player: &mut AnimationPlayer, turn: &mut Turn) {
    if let Some(old) = turn.fade.take() {
        player.stop(old.node);
    }
}

/// One engine arm, `0x7121a0(bone −1, id, variation −1, offset, 1.0, blend, 1)`. The id resolves
/// through the model's fallback chain (`0x711bf0` at `0x71245d`); a variation is rolled and the
/// play count drawn (`0x712692`). False only when nothing resolves (`0x712484`).
fn play_arm(
    player: &mut AnimationPlayer,
    anims: &benilla_assets::ModelAnimations,
    catalog: &benilla_formats::AnimDataCatalog,
    rng: &mut benilla_assets::AnimRng,
    turn: &mut Turn,
    id: u16,
    blend: bool,
    offset: f32,
    now: f64,
) -> bool {
    let outgoing = turn.playing;
    let res = anims.resolve(id, catalog);
    let Some(clip) = anims.pick_variation(res.id, rng.draw()) else {
        return false;
    };
    let replays = rng.replay_count(clip.replay);
    let blend_time = clip.blend_time.max(0.0);
    // The primary's record (`[blk+0xa4]`), on either of its clip's tracks.
    let same = outgoing.is_some_and(|n| n == clip.node || Some(n) == clip.blend_node);
    if !blend {
        // No blend: the secondary slot is cleared (`0x712635`, `[blk+0xd0] = −1`).
        clear_fade(player, turn);
    } else if same && turn.completed {
        // The shortcut (`0x712553` → `0x71262d`): the sequence that ran to its end is written
        // again as the primary alone; the secondary and its window stay as they are.
    } else if turn
        .fade
        .is_none_or(|f| crate::creature_anim::select::blend_lambda(fade_frac(&f, now)) <= 0.5)
    {
        // The copy (`0x7125d6`): the outgoing primary becomes the secondary, its clock kept, over
        // the incoming clip's blendTime (`0x7125f2`). A zero blendTime ends it on the next pass.
        clear_fade(player, turn);
        turn.fade = outgoing.filter(|_| blend_time > 0.0).map(|node| Fade {
            node,
            until: now + f64::from(blend_time),
            span: blend_time,
        });
    }
    // Else the half-blend refusal (`0x7125c9`/`0x7125d4`): λ > 0.5 keeps the secondary and its
    // window, and the primary alone is written.
    // The primary runs on the clip's track the secondary is not on, so a re-arm of the sequence
    // the secondary holds leaves that clock alone.
    let node = match (turn.fade, clip.blend_node) {
        (Some(f), Some(twin)) if f.node == clip.node => twin,
        (Some(f), None) if f.node == clip.node => {
            // A model built with one track per clip: the arm takes it, and the secondary ends.
            turn.fade = None;
            clip.node
        }
        _ => clip.node,
    };
    // The primary write replaces the outgoing primary (`0x712692`), unless the copy kept it.
    if let Some(out) = outgoing.filter(|&o| o != node && turn.fade.is_none_or(|f| f.node != o)) {
        player.stop(out);
    }
    let active = player.start(node);
    active.set_repeat(if clip.looping {
        RepeatAnimation::Forever
    } else {
        RepeatAnimation::Never
    });
    active.set_speed(1.0);
    active.seek_to(offset);
    // λ = 1 on a blended arm's first frame, so the pose does not move on the frame it lands.
    active.set_weight(if turn.fade.is_some() { 0.0 } else { 1.0 });
    turn.playing = Some(node);
    turn.completed = false;
    turn.ends = Some(now - f64::from(offset) + f64::from(clip.duration) * f64::from(replays));
    true
}

/// Advance the fade: λ, smoothstep of the window still to run (`0x714880`), weights the secondary
/// and the primary takes `1 − λ`. At the window's end (`0x7147b1`) the secondary alone stops.
fn advance_fade(player: &mut AnimationPlayer, turn: &mut Turn, now: f64) {
    let Some(fade) = turn.fade else {
        return;
    };
    let frac = fade_frac(&fade, now);
    let lambda = if frac <= 0.0 {
        turn.fade = None;
        player.stop(fade.node);
        0.0
    } else {
        let lambda = crate::creature_anim::select::blend_lambda(frac);
        if let Some(out) = player.animation_mut(fade.node) {
            out.set_weight(lambda);
        }
        lambda
    };
    if let Some(cur) = turn.playing.and_then(|n| player.animation_mut(n)) {
        cur.set_weight(1.0 - lambda);
    }
}

/// One frame of a copy's key bone: play the engine's arm if it is new, advance the fade, and
/// return the arm whose window just closed, for the completion callback.
fn step_copy(
    player: &mut AnimationPlayer,
    anims: &benilla_assets::ModelAnimations,
    catalog: &benilla_formats::AnimDataCatalog,
    rng: &mut benilla_assets::AnimRng,
    turn: &mut Turn,
    arm: PaneArm,
    now: f64,
) -> Option<u64> {
    if turn.played != Some(arm.serial) {
        // A fresh copy's arm blends from the idle Stand its load armed (`0x71d5d0` → `0x70ebd0`).
        let offset = arm.offset_ms as f32 / 1000.0;
        if !play_arm(
            player,
            anims,
            catalog,
            rng,
            turn,
            arm.anim_id,
            arm.blend,
            offset,
            now,
        ) {
            // Nothing resolved, nothing armed: no end is owed.
            turn.ends = None;
        }
        turn.played = Some(arm.serial);
    }
    advance_fade(player, turn, now);
    let end = turn.ends?;
    (now >= end).then(|| {
        turn.ends = None;
        turn.completed = true;
        arm.serial
    })
}

/// The subjects' world: who a GUID is, what a unit wears, the dressing room's assembly.
#[derive(SystemParam)]
pub(super) struct PaneWorld<'w, 's> {
    index: Res<'w, crate::net::GuidIndex>,
    nets: Query<'w, 's, &'static NetEntity>,
    me: Query<'w, 's, Entity, With<SelfPlayer>>,
    look: DressedLook<'w, 's>,
    creatures: Option<Res<'w, Creatures>>,
    bake: Res<'w, DressUpBake>,
}

/// The render resources a pane build spends.
#[derive(SystemParam)]
pub(super) struct PaneRender<'w> {
    images: ResMut<'w, Assets<Image>>,
    mats: ResMut<'w, Assets<WowModelMaterial>>,
    palettes: ResMut<'w, benilla_world::rig_palette::RigPalettes>,
    lights: PaneLights<'w>,
    anim_data: Option<Res<'w, crate::creature_anim::AnimData>>,
    rng: ResMut<'w, benilla_assets::AnimRng>,
    time: Res<'w, Time<bevy::time::Real>>,
    gx: Res<'w, GxAspect>,
    displays: ResMut<'w, PaneDisplays>,
}

/// The per-frame pass over the engine's character panes: a booth per visible pane, the copy the
/// engine asked for built into it, its arms played and its ends reported.
pub(super) fn sync_panes(
    mut commands: Commands,
    feed: Res<crate::ui_panes::CharacterPanes>,
    mut answers: ResMut<crate::ui_panes::PaneAnswers>,
    mut state: ResMut<PaneBooths>,
    requests: Res<PaneRequests>,
    mut booths: ResMut<Booths>,
    mut marks: ResMut<BoothPanes>,
    world: PaneWorld,
    mut render: PaneRender,
    mut cams: Query<(&mut Transform, &mut Projection, &mut RenderTarget), With<BoothCam>>,
    mut rigs: Query<(&mut AnimationPlayer, &benilla_assets::ModelAnimations)>,
    mut emitters: Query<(
        &mut benilla_world::particles::ParticleEmitter,
        &RenderLayers,
    )>,
) {
    let state = &mut *state;
    state.drawn.clear();
    let (session, panes) = (feed.session, &feed.rows);
    answers.session = session;
    // A pane that left the list (hidden, destroyed) or draws a file drops its booth: the hide hook
    // drops the copy (`0x505ce0`), and a `SetModel` file replaces it. A file pane keeps one while
    // a `SetUnit` over it is pending, whose copy replaces the file. A new VM drops them all: its
    // arena reissues the old one's handles.
    let new_vm = state.session != session;
    state.session = session;
    if new_vm {
        state.kept.clear();
    }
    let wanted = |p: &CharacterPaneFrame| !p.file || p.pending;
    let keep: Vec<FrameHandle> = panes
        .iter()
        .filter(|p| !new_vm && wanted(p))
        .map(|p| p.handle)
        .collect();
    let gone: Vec<FrameHandle> = state
        .panes
        .keys()
        .filter(|h| !keep.contains(h))
        .copied()
        .collect();
    for h in gone {
        if let Some(pane) = state.panes.remove(&h) {
            if let (false, Some(booth)) = (new_vm, booths.0.get(&pane.key)) {
                state.kept.insert(h, (pane.size, booth.target.clone()));
            }
            retire(
                &mut commands,
                pane,
                &mut booths,
                &mut marks,
                &mut state.free_layers,
            );
        }
    }
    let now = render.time.elapsed_secs_f64();
    let me = world.me.single().ok();
    let mut wanted_displays: Vec<u32> = Vec::new();
    for pane in panes.iter().filter(|p| wanted(p)) {
        let Some(req) = requests.get(session, pane.handle) else {
            continue; // not extracted yet: next frame
        };
        if req.size_px.x == 0 || req.size_px.y == 0 {
            continue;
        }
        let size = target_size(req.size_px);
        let kept = state
            .kept
            .remove(&pane.handle)
            .filter(|(s, _)| *s == size)
            .map(|(_, image)| image);
        let booth_pane = state.panes.entry(pane.handle).or_insert_with(|| {
            let layer = state.free_layers.pop().unwrap_or_else(|| {
                let l = PANE_LAYER_BASE + state.next_layer;
                state.next_layer += 1;
                l
            });
            open(
                &mut commands,
                &mut render.images,
                &mut booths,
                pane,
                layer,
                size,
                kept,
            )
        });
        let Some(booth) = booths.0.get_mut(&booth_pane.key) else {
            continue;
        };
        // The target follows the pane's size.
        if booth_pane.size != size {
            booth_pane.size = size;
            let image = render.images.add(new_target_image_sized(size.x, size.y));
            if let Ok((_, _, mut target)) = cams.get_mut(booth_pane.camera) {
                *target = RenderTarget::Image(image.clone().into());
            }
            booth.target = image;
            booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
        }
        let aspect = size.x as f32 / size.y as f32;
        // The view through the copy's camera; the target's pixels per device pixel scale the
        // orthographic ladder of a capped target.
        let display_aspect = render.gx.0;
        let view_of = move |anchors: &PortraitAnchors| {
            body_view(
                anchors,
                pane.camera,
                pane.facing,
                display_aspect,
                &req.units,
                size.as_vec2(),
                size.x as f32 / req.size_px.x as f32,
            )
        };

        // ── The subject the latest request names ────────────────────────────────────────
        let subject = if pane.guid != 0 {
            Some(Subject::Unit(pane.guid))
        } else if pane.display != 0 {
            Some(Subject::Display(pane.display))
        } else {
            None
        };
        // A display subject is mirrored off a stand-in the display cache builds.
        let want_display = match subject {
            Some(Subject::Display(d)) => Some(d),
            _ => None,
        };
        sync_stand_in(
            &mut commands,
            booth_pane,
            want_display,
            world.creatures.as_deref(),
        );
        wanted_displays.extend(want_display);

        // ── Build: a new request, or a re-dress of the copy that stands ─────────────────
        let fresh_request =
            pane.pending && booth_pane.built.as_ref().map_or(0, |b| b.build) < pane.builds;
        // A copy spawned this frame has no player yet: the root still holds the old one until
        // the commands apply, so its arms wait a frame.
        let mut spawned = false;
        let scene = render.lights.scene(pane.light, pane.fog);
        if let Some(subject) = subject.filter(|_| pane.builds > 0) {
            // A GUID the client holds no unit for builds nothing, and the request is spent
            // (`0x468460` at `0x505b8d`, `0x505d30`): the old copy stays until the next one.
            if fresh_request {
                if let Subject::Unit(guid) = subject {
                    if !world.index.0.contains_key(&guid) {
                        answers.missed.push((pane.handle, pane.builds));
                    }
                }
            }
            let source = resolve(&world, subject, pane.kind, me, booth_pane);
            let current = source.as_ref().map(|s| look_key(&world, s));
            let stale = match (&booth_pane.built, &current) {
                // The copy stands for this request: a re-dress (benilla's stand-in for the stock
                // windows' `UNIT_MODEL_CHANGED` → `SetUnit`) or a new light rebuilds it.
                (Some(b), Some(look)) if b.build == pane.builds => {
                    b.look != *look || b.scene != scene
                }
                _ => fresh_request,
            };
            if let (true, Some(source), Some(look)) = (stale, source, current) {
                if let Some(anchors) = build(
                    &mut commands,
                    &mut render,
                    booth,
                    booth_pane,
                    &world,
                    &source,
                    scene,
                ) {
                    booth_pane.view = None;
                    place(&mut commands, booth_pane, booth, view_of(&anchors));
                    booth_pane.built = Some(Built {
                        build: pane.builds,
                        look,
                        scene,
                        anchors,
                    });
                    spawned = true;
                    // The copy goes to `+0x318` with Stand armed (`0x5059a0`). A re-dress or a
                    // new light keeps the engine's copy, whose arm the fresh player replays.
                    if fresh_request {
                        answers.built.push((pane.handle, pane.builds));
                    }
                }
            }
        }

        // ── Draw: the copy the engine holds, posed, played and composited ───────────────────
        let Some((built_for, view)) = booth_pane
            .built
            .as_ref()
            .map(|b| (b.build, view_of(&b.anchors)))
        else {
            continue;
        };
        let Some(copy) = pane.copy.filter(|c| c.build <= built_for) else {
            continue; // the engine holds no copy (`ClearModel`): nothing at `+0x318`
        };
        // The root, the widget's `T(pos)·R(facing)·S(s)`, and the camera it is seen through.
        place(&mut commands, booth_pane, booth, view);
        // The arms, once the copy's player is up (the frame after its spawn).
        if let (false, Ok((mut player, anims)), Some(anim_data)) = (
            spawned,
            rigs.get_mut(booth.root),
            render.anim_data.as_deref(),
        ) {
            let finished = step_copy(
                &mut player,
                anims,
                &anim_data.0,
                &mut render.rng,
                &mut booth_pane.turn,
                copy.arm,
                now,
            );
            if let Some(serial) = finished {
                answers.finished.push((pane.handle, copy.build, serial));
            }
        }
        marks.0.insert(booth_pane.key.clone(), aspect);
        state.drawn.push(pane.handle);
    }
    // The panes that hold a booth but drew nothing this frame keep their camera asleep.
    for (h, pane) in &state.panes {
        if !state.drawn.contains(h) {
            marks.0.remove(&pane.key);
        }
    }
    // A particle's half-extent unit follows the leg its pane draws through, for the emitters of
    // the copy that stands and of any spawned since (`0x7b2a50`).
    let stars: Vec<(usize, f32)> = state
        .panes
        .values()
        .filter_map(|p| Some((p.layer, p.view.as_ref()?.star)))
        .collect();
    if !stars.is_empty() {
        for (mut emitter, layers) in &mut emitters {
            let star = stars
                .iter()
                .find(|(layer, _)| layers.intersects(&RenderLayers::layer(*layer)))
                .map(|&(_, star)| star);
            if let Some(star) = star.filter(|&s| s != emitter.size_scale()) {
                emitter.set_size_scale(star);
            }
        }
    }
    wanted_displays.sort_unstable();
    wanted_displays.dedup();
    if render.displays.0 != wanted_displays {
        render.displays.0 = wanted_displays;
    }
}

/// A pane's target size: its device-pixel rect, scaled down whole to fit [`PANE_TARGET_MAX`].
fn target_size(px: UVec2) -> UVec2 {
    let longest = px.max_element();
    if longest <= PANE_TARGET_MAX {
        return px;
    }
    let k = PANE_TARGET_MAX as f32 / longest as f32;
    (px.as_vec2() * k).round().as_uvec2().max(UVec2::ONE)
}

/// Stand a pane's booth up: a target of the pane's size on a layer of its own, a model root and a
/// transparent camera.
fn open(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    booths: &mut Booths,
    pane: &CharacterPaneFrame,
    layer: usize,
    size: UVec2,
    kept: Option<Handle<Image>>,
) -> PaneBooth {
    let key = format!("pane:{:?}", pane.handle);
    let image = kept.unwrap_or_else(|| images.add(new_target_image_sized(size.x, size.y)));
    let layers = RenderLayers::layer(layer);
    let root = commands
        .spawn((
            Name::new(format!("pane booth root {key}")),
            Transform::IDENTITY,
            Visibility::Visible,
            layers.clone(),
        ))
        .id();
    let camera = commands
        .spawn((
            Name::new(format!("pane booth camera {key}")),
            super::booth_view_shape(),
            Camera {
                order: PANE_CAMERA_ORDER,
                // Transparent: a pane draws only its model; the window's art shows behind it.
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            // No glow: the reference paints a model frame after the WorldFrame's FFX.
            benilla_world::ffx_glow::FfxGlow::UI_PANE,
            // A placeholder until the first copy aims it.
            Projection::from(PerspectiveProjection {
                fov: PORTRAIT_FOV,
                near: 0.02,
                far: 100.0,
                ..default()
            }),
            layers.clone(),
            BoothCam(key.clone()),
        ))
        .id();
    booths
        .0
        .insert(key.clone(), Booth::new(layers, root, image));
    PaneBooth {
        key,
        name: pane.name.clone(),
        layer,
        camera,
        size,
        view: None,
        built: None,
        stand_in: None,
        turn: Turn::default(),
    }
}

/// Take a pane's booth down: its root and everything on it, its camera, its stand-in and layer.
fn retire(
    commands: &mut Commands,
    pane: PaneBooth,
    booths: &mut Booths,
    marks: &mut BoothPanes,
    free_layers: &mut Vec<usize>,
) {
    if let Some(booth) = booths.0.remove(&pane.key) {
        commands.entity(booth.root).despawn();
    }
    commands.entity(pane.camera).despawn();
    if let Some(stand_in) = pane.stand_in {
        commands.entity(stand_in.entity).despawn();
    }
    marks.0.remove(&pane.key);
    free_layers.push(pane.layer);
}

/// Keep a booth's stand-in on the display its subject names, rebuilding it when that changes.
fn sync_stand_in(
    commands: &mut Commands,
    pane: &mut PaneBooth,
    want: Option<u32>,
    creatures: Option<&Creatures>,
) {
    if pane.stand_in.as_ref().map(|s| s.display) != want {
        if let Some(old) = pane.stand_in.take() {
            commands.entity(old.entity).despawn();
        }
        pane.stand_in = want.map(|display| StandIn {
            display,
            entity: commands
                .spawn((
                    PortraitStandIn(display),
                    // Hidden: the booth draws its own copies.
                    Transform::default(),
                    Visibility::Hidden,
                ))
                .id(),
            built: false,
        });
    }
    let Some(stand_in) = pane.stand_in.as_mut().filter(|s| !s.built) else {
        return;
    };
    // `None` while the display's model loads.
    let Some((parts, cards)) = creatures.and_then(|c| c.display_mirror(stand_in.display)) else {
        return;
    };
    commands.entity(stand_in.entity).with_children(|kids| {
        for part in parts {
            kids.spawn((Transform::default(), Visibility::Inherited, part));
        }
        for card in cards {
            kids.spawn((Transform::default(), Visibility::Inherited, card));
        }
    });
    stand_in.built = true;
}

/// Where a copy's look comes from.
enum Source {
    /// A unit or stand-in entity whose dressed look is mirrored.
    Mirror(Entity),
    /// The dressing room's assembly (`DressUpModel` on the player).
    Assembled,
}

/// The look a subject's copy is built from; `None` for a GUID the client holds no unit for, or a
/// model still loading.
fn resolve(
    world: &PaneWorld,
    subject: Subject,
    kind: FrameKind,
    me: Option<Entity>,
    pane: &PaneBooth,
) -> Option<Source> {
    match subject {
        Subject::Unit(guid) => {
            let unit = *world.index.0.get(&guid)?;
            // The dressing room on the player wears the room's try-ons, an assembly of the
            // player's own look, not a mirror of the player.
            if kind == FrameKind::DressUpModel && Some(unit) == me {
                return (!world.bake.parts.is_empty()).then_some(Source::Assembled);
            }
            Some(Source::Mirror(unit))
        }
        Subject::Display(display) => pane
            .stand_in
            .as_ref()
            .filter(|s| s.display == display && s.built)
            .map(|s| Source::Mirror(s.entity)),
    }
}

/// The look key of `source`.
fn look_key(world: &PaneWorld, source: &Source) -> Look {
    match source {
        Source::Mirror(unit) => {
            let (parts, riders, billboards, effects) = world.look.collect(*unit);
            let (dress, rev) = world.look.snapshot_inputs(*unit);
            Look::Mirror(SnapKey::build(
                *unit,
                &parts,
                &riders,
                &billboards,
                &effects,
                dress,
                rev,
            ))
        }
        Source::Assembled => Look::Assembled(world.bake.revision),
    }
}

/// Spawn `source`'s copy into the booth; `None`, touching nothing, while its display or a
/// material is not ready, so it retries next frame.
fn build(
    commands: &mut Commands,
    render: &mut PaneRender,
    booth: &mut Booth,
    pane: &mut PaneBooth,
    world: &PaneWorld,
    source: &Source,
    scene: Option<PaneScene>,
) -> Option<PortraitAnchors> {
    let creatures = world.creatures.as_deref();
    let mut relight = |m: &Handle<WowModelMaterial>, ok: &mut bool| {
        render
            .lights
            .twin(scene, m, &mut render.mats)
            .unwrap_or_else(|| {
                *ok = false;
                m.clone()
            })
    };
    let mut ok = true;
    let (display_id, parts, riders, billboards, effects, grip) = match source {
        Source::Mirror(unit) => {
            let (parts, riders, billboards, effects) = world.look.collect(*unit);
            if parts.is_empty() {
                return None; // the unit's model has not attached
            }
            let display_id = world
                .nets
                .get(*unit)
                .ok()
                .and_then(|n| n.display_id)
                .or_else(|| world.look.stand_in_display(*unit));
            let grip = hand_grip(&riders, &billboards, &effects);
            log_bake(&pane.key, "pane", &parts, &riders, &billboards, &effects);
            let booth_parts: Vec<BoothPart> = parts
                .iter()
                .map(|p| BoothPart {
                    skinned: p.skinned_mesh.clone(),
                    static_mesh: p.static_mesh.clone(),
                    material: relight(&p.material, &mut ok),
                    alpha_anim: None,
                    twins: BoothTwins::default(),
                    mat_anim: false,
                })
                .collect();
            let booth_riders: Vec<BoothRider> = riders
                .iter()
                .map(|r| BoothRider {
                    mesh: r.static_mesh.clone(),
                    material: relight(&r.material, &mut ok),
                    bone: r.bone,
                    offset: r.offset,
                    twins: BoothTwins::default(),
                    mat_anim: false,
                })
                .collect();
            let booth_billboards: Vec<BoothBillboardSpec> = billboards
                .iter()
                .map(|b| BoothBillboardSpec {
                    mesh: b.mesh.clone(),
                    material: relight(&b.material, &mut ok),
                    bone: b.bone,
                    offset: b.seat.offset(),
                    kind: b.kind,
                    twins: BoothTwins::default(),
                    mat_anim: false,
                })
                .collect();
            let booth_effects: Vec<BoothEffects> = effects
                .iter()
                .map(|fx| BoothEffects {
                    bone: fx.bone,
                    offset: fx.offset,
                    emitters: fx.emitters.clone(),
                    rig: fx.rig.clone(),
                })
                .collect();
            (
                display_id,
                booth_parts,
                booth_riders,
                booth_billboards,
                booth_effects,
                grip,
            )
        }
        Source::Assembled => {
            let bake = &world.bake;
            let booth_parts: Vec<BoothPart> = bake
                .parts
                .iter()
                .map(|p| BoothPart {
                    skinned: p.skinned_mesh.clone(),
                    static_mesh: p.static_mesh.clone(),
                    material: relight(&p.material, &mut ok),
                    alpha_anim: None,
                    twins: BoothTwins::default(),
                    mat_anim: false,
                })
                .collect();
            let booth_riders: Vec<BoothRider> = bake
                .riders
                .iter()
                .map(|r| BoothRider {
                    mesh: r.mesh.clone(),
                    material: relight(&r.material, &mut ok),
                    bone: r.bone,
                    offset: r.offset,
                    twins: BoothTwins::default(),
                    mat_anim: false,
                })
                .collect();
            let booth_billboards: Vec<BoothBillboardSpec> = bake
                .billboards
                .iter()
                .map(|b| BoothBillboardSpec {
                    mesh: b.mesh.clone(),
                    material: relight(&b.material, &mut ok),
                    bone: b.bone,
                    offset: b.offset,
                    kind: b.kind,
                    twins: BoothTwins::default(),
                    mat_anim: false,
                })
                .collect();
            let booth_effects: Vec<BoothEffects> = bake
                .effects
                .iter()
                .map(|fx| BoothEffects {
                    bone: fx.bone,
                    offset: fx.offset,
                    emitters: fx.emitters.clone(),
                    rig: fx.rig.clone(),
                })
                .collect();
            (
                // 0 is no display: an empty bake.
                (bake.display_id != 0).then_some(bake.display_id),
                booth_parts,
                booth_riders,
                booth_billboards,
                booth_effects,
                bake.grip,
            )
        }
    };
    // Anchors and materials before any teardown: a still-loading display or a twin on the world
    // light would latch wrong.
    let anchors = booth_anchors(creatures, display_id)?;
    if !ok {
        booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
        return None;
    }
    let rig = creatures
        .zip(display_id)
        .and_then(|(c, d)| c.display_rig(d));
    commands.entity(booth.root).despawn_related::<Children>();
    let mut booth_rig = spawn_booth_model(
        commands,
        &mut render.palettes,
        booth.root,
        booth.layer.clone(),
        &parts,
        &riders,
        rig.as_ref().and_then(|r| {
            r.inverse_bindposes
                .as_ref()
                .map(|ibp| (r.skeleton, ibp, r.animations))
        }),
        render.anim_data.as_deref().map(|a| &a.0),
        // The copy animates: the reference's pane renders live.
        BoothMotion::Loop,
        grip,
        &billboards,
        BoothInstance::default(),
    );
    // The worn items' effects, a live pane's alone.
    spawn_booth_effects(
        commands,
        &mut booth_rig,
        &booth.layer,
        render.lights.buffer(scene).as_ref(),
        &effects,
        BoothInstance::default(),
    );
    // The spawn plays the idle the load arms, Stand variation 0 (`0x70ebd0`); the engine's first
    // arm blends from it.
    pane.turn = Turn {
        playing: rig.as_ref().and_then(|r| r.animations).and_then(|a| {
            let stand = render
                .anim_data
                .as_deref()
                .map_or(ANIM_STAND, |d| a.resolve(ANIM_STAND, &d.0).id);
            a.find(stand).map(|c| c.node)
        }),
        ..Turn::default()
    };
    booth.live = true;
    booth.rigged = booth_rig.rigged();
    booth_rig.finish(commands);
    booth.parked = false;
    wake_booth(
        booth,
        &render.mats,
        parts
            .iter()
            .map(|p| &p.material)
            .chain(riders.iter().map(|r| &r.material))
            .chain(billboards.iter().map(|b| &b.material)),
    );
    Some(anchors)
}

/// Pose a pane's root and aim its camera at `view`, unless they hold it. The aim goes in as a
/// command, so a camera spawned through `Commands` this pass takes it with its spawn and the pane's
/// first frame composites an aimed view; `try_insert` leaves a camera already despawned alone.
fn place(commands: &mut Commands, pane: &mut PaneBooth, booth: &mut Booth, view: BodyView) {
    if pane.view.as_ref() == Some(&view) {
        return;
    }
    commands.entity(booth.root).insert(view.root);
    commands
        .entity(pane.camera)
        .try_insert((view.camera, view.bevy_projection()));
    booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
    pane.view = Some(view);
}

/// The composite: each drawn pane's target at the pane's rect, premultiplied, at the frame's own
/// alpha (`0x76d120`).
pub(super) fn compose_panes(
    state: Res<PaneBooths>,
    requests: Res<PaneRequests>,
    booths: Res<Booths>,
    mut quads: ResMut<UiQuads>,
) {
    quads
        .overlays
        .extend(pane_quads(&state, &requests, &booths));
}

/// [`compose_panes`]'s pure half.
fn pane_quads(state: &PaneBooths, requests: &PaneRequests, booths: &Booths) -> Vec<UiQuad> {
    state
        .drawn
        .iter()
        .filter_map(|h| {
            let req = requests.get(state.session, *h)?;
            let booth = booths.0.get(&state.panes.get(h)?.key)?;
            Some(UiQuad {
                rect: req.rect,
                z_key: req.z_key,
                texture: Some(booth.target.clone()),
                color: [1.0, 1.0, 1.0, req.alpha],
                // A render target is premultiplied.
                premultiplied: true,
                clip: req.clip,
                ..default()
            })
        })
        .collect()
}

impl PaneBooths {
    /// The booth key of the pane the global `name` names, for the dump instrument.
    pub(super) fn key_named(&self, name: &str) -> Option<&str> {
        self.panes
            .values()
            .find(|p| p.name.as_deref() == Some(name))
            .map(|p| p.key.as_str())
    }

    /// The target the pane `h` draws, while its booth stands.
    #[cfg(test)]
    pub(crate) fn target(&self, h: FrameHandle, booths: &Booths) -> Option<Handle<Image>> {
        Some(booths.0.get(&self.panes.get(&h)?.key)?.target.clone())
    }
}

#[cfg(test)]
#[path = "panes_tests.rs"]
mod tests;
