//! The world click as the mouse commands make it, inside their calls: `CameraOrSelectOrMoveStart`
//! and `TurnOrActionStart` arm it with the pick under the cursor (`ArmClick 0x514810` →
//! `PickObjectUnderCursor 0x481f00` → `PickAtCursor 0x481190`), and the last primary's release
//! dispatches it (`ReleaseCommands 0x514b70` → `DispatchWorldClick 0x481f60`) and runs its legs
//! before the command returns. The VM keeps the command bits and hands each step to the call host
//! ([`benilla_ui::script::ClickCall`]), which applies it with the click's legs ([`ClickLegs`]), so
//! Lua after the command, in the same binding body or a hook around it, reads the world as the
//! click left it.
//!
//! The arm picks inside its call, whatever ran the command (a press's binding, a key's, a
//! handler's): the input records' walk lends the call host with the pick's tracer ([`ArmTracer`]),
//! which traces the world, never a plate over it, at the cursor of the record that runs
//! ([`benilla_ui::script::ClickPress`]), the reference's stored cursor (`[frame+0xa0]+0x1118`,
//! `0x481f1d`). An arm made under a host without
//! the tracer (a signal point inside another call's apply, the stuck-latch sweep's up-half) is
//! traced where the record's walk returns, and the steps after it wait for that trace and apply
//! there, in order. A nameplate's click is a call of its own, applied with the same legs inside the
//! click ([`benilla_ui::script::ScriptCall::PlateClick`]); what no call dispatches, a crate's
//! `Interact`, runs where the frame's records end ([`WorldClickLegs`]).

use bevy::ecs::schedule::ScheduleLabel;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{ClickCall, ClickMode, ClickPress};

use super::PressPick;
use crate::player::PressGesture;
use crate::ui_script::{InputRecord, Record};

/// The click a mouse command armed: its mode (`CInputControl+0x18`) and the press's gesture, the
/// press time (`+0x14`) and the travel since (`+0x08`, `+0x0c`), which the release's drag test
/// reads (`0x514ae0`). The look session adds the travel.
#[derive(Resource, Default)]
pub(crate) struct ClickArm {
    armed: Option<(ClickMode, PressGesture)>,
    /// The pick the tracer traced inside the arm's call ([`ArmTracer`]), for the arm to take.
    traced: Option<PressPick>,
    /// An arm took a pick since the frame's records began or the last record's world steps
    /// ([`world_steps`]), which then leave the press's plate alone.
    latched: bool,
    /// An arm found no pick to take: it is traced where the record's walk returns, and the steps
    /// made after it wait for it.
    untraced: bool,
}

impl ClickArm {
    /// Mode 0 stored: nothing dispatches at the release.
    pub(crate) fn disarm(&mut self) {
        self.armed = None;
        self.traced = None;
        self.untraced = false;
    }

    /// One motion's camera travel, while a click is armed (`0x514400`).
    pub(crate) fn add_travel(&mut self, yaw: f32, pitch: f32) {
        if let Some((_, gesture)) = &mut self.armed {
            gesture.add_travel(yaw, pitch);
        }
    }

    /// Arm `mode` with a press at `now`, as a test's release finds it, the pick left as it stands.
    #[cfg(test)]
    pub(crate) fn arm_at(&mut self, mode: ClickMode, now: f32) {
        self.armed = Some((mode, PressGesture::new(now)));
    }
}

/// What no call dispatches, the click dispatcher's order kept: a crate's `Interact`, and the casts
/// its dispatchers queued. Run where the frame's records end.
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct WorldClickLegs;

crate::script_calls::disjoint::disjoint_set! {
    /// The world click's state, as its call applies it: the arm and its pick, then the
    /// dispatcher's legs in its order (`0x481f60`), and a nameplate's click's, which takes the
    /// object legs without the router (`0x4949f0`). Each is lent alone: they share the press pick,
    /// the selection and the cast ladder. A leg whose state a harness lacks is absent and does
    /// nothing.
    pub(crate) struct ClickLegs, ClickLegsState {
        /// The armed click and the pick at its press.
        arm: Option<ArmLeg<'static, 'static>>,
        /// The right-click's payload leg.
        payload: Option<super::click::PayloadLeg<'static, 'static>>,
        /// The left click's object leg ahead of its select (`0x4925d0`): the trade and the pet's
        /// feed.
        trade: Option<crate::ui_action::drop_item::ClickTrade<'static, 'static>>,
        feed: Option<crate::ui_action::drop_item::ClickFeed<'static, 'static>>,
        /// The left click's select, then its terrain and object commits.
        select: Option<super::click::ClickSelect<'static, 'static>>,
        commits: Option<crate::spell::targeting::ClickCommits<'static, 'static>>,
        /// The right-click's router past its payload leg.
        dispatch: Option<super::click::ClickDispatch<'static, 'static>>,
        /// The casts the dispatchers queued.
        casts: Option<crate::ui_action::ClickCastDrain<'static, 'static>>,
        /// A nameplate's click: its unit, its greeting and its `SelectTarget`, and the pick its
        /// right button's interact acts on.
        plate: Option<super::click::PlateLeg<'static, 'static>>,
        plate_pick: Option<super::click::PlatePick<'static, 'static>>,
    }
}

/// The armed click and the pick at its press.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ArmLeg<'w, 's> {
    arm: ResMut<'w, ClickArm>,
    press: ResMut<'w, PressPick>,
    time: Res<'w, Time>,
    cover: Option<Res<'w, crate::loading_screen::LoadingScreen>>,
    inspect: Option<Res<'w, crate::ui_script::InspectMode>>,
    window: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    motion: Option<Res<'w, AccumulatedMouseMotion>>,
    look: Option<Res<'w, crate::player::LookConfig>>,
}

impl ArmLeg<'_, '_> {
    /// The world enter behind the cover releases everything (`0x5144c0`): a gesture in flight is
    /// dropped, so the release clicks nothing in the world just left.
    fn covering(&self) -> bool {
        self.cover.as_deref().is_some_and(|c| c.covering())
    }

    /// Whether an arm of `mode` at `press` stores: a left press the payload's drop consumed, or
    /// the inspector's, selects nothing.
    pub(crate) fn admits(&self, mode: ClickMode, press: ClickPress) -> bool {
        let refused = mode == ClickMode::Select
            && (press.eaten || self.inspect.as_deref().is_some_and(|i| i.enabled));
        !(self.covering() || refused)
    }

    /// Mode 0 stored.
    pub(crate) fn disarm(&mut self) {
        self.arm.disarm();
    }

    /// The pick traced inside the arm's call (`0x51482d` → `0x481f00`), for its store.
    pub(crate) fn traced(&mut self, pick: PressPick) {
        self.arm.traced = Some(pick);
    }

    /// `ArmClick`'s store of `mode` (`0x514827`): the pick under the cursor (`0x481f00`), as
    /// the tracer traced it inside the call ([`Self::traced`]), and the press time (`0x514910`).
    pub(crate) fn arm(&mut self, mode: ClickMode) {
        match self.arm.traced.take() {
            Some(pick) => {
                *self.press = pick;
                self.arm.latched = true;
                self.arm.untraced = false;
            }
            None => self.arm.untraced = true,
        }
        let now = self.time.elapsed_secs();
        self.arm.armed = Some((mode, PressGesture::new(now)));
    }

    /// Whether `step` waits for an arm's pick to be traced: a release of an arm that found none,
    /// or any step made after one already `waiting`.
    pub(crate) fn waits(&self, waiting: bool, step: ClickCall) -> bool {
        waiting || (self.arm.untraced && step == ClickCall::Release)
    }

    /// The last primary's release (`0x514c35`): the armed click's mode when it dispatches, which it
    /// does unless the release is a drag (`0x514ae0`, `0x514d4b`) or the cover is up.
    pub(crate) fn release(&mut self) -> Option<ClickMode> {
        let (mode, mut gesture) = self.arm.armed.take()?;
        // The frame's motion, which reaches the look session only after the records: the
        // reference's motion messages reach `0x514400` ahead of the release's test.
        let (yaw, pitch) = self.frame_travel();
        gesture.add_travel(yaw, pitch);
        let now = self.time.elapsed_secs();
        (!self.covering() && gesture.is_click(now)).then_some(mode)
    }

    /// The camera travel the frame's mouse motion charges a click, as the look session charges it:
    /// none while the window lacks the focus, whose motion is the pointer travelling in another
    /// app.
    fn frame_travel(&self) -> (f32, f32) {
        let focused = self.window.single().is_ok_and(|w| w.focused);
        match (self.motion.as_deref(), self.look.as_deref()) {
            (Some(motion), Some(look)) if focused => look.click_travel(motion.delta),
            _ => (0.0, 0.0),
        }
    }
}

crate::script_calls::disjoint::disjoint_set! {
    /// `PickObjectUnderCursor`'s trace (`0x481f00` → `PickAtCursor 0x481190` at `0x481f47`) as a
    /// param: the targeting word published to the pick's readers (`0x481050`), then the pick at
    /// the point. Each is lent alone: the pick reads what the words write.
    pub(crate) struct ArmTracer, ArmTracerState {
        words: crate::spell::targeting::PickWords<'static>,
        pick: super::hover::PointPick<'static, 'static>,
    }
}

impl ArmTracer<'_, '_> {
    /// The arm's pick at `at`, in window coordinates (the window's cursor or the probe's aim with
    /// none), against the targeting word as it stands now: the world under the cursor, whatever
    /// plate covers it ([`super::hover::PointPick::world_at`]).
    pub(crate) fn trace(&mut self, at: Option<Vec2>) -> PressPick {
        self.pick_with(at, false)
    }

    /// The pick at `at` against the targeting word as it stands now; with `plate`, a plate the
    /// walk left hovered is its unit.
    fn pick_with(&mut self, at: Option<Vec2>, plate: bool) -> PressPick {
        self.words().publish();
        // `WOW_PICK_COST=1` logs each pick's cost.
        static METER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let t0 = METER
            .get_or_init(|| std::env::var("WOW_PICK_COST").as_deref() == Ok("1"))
            .then(std::time::Instant::now);
        let mut point = self.pick();
        let pick = if plate {
            point.at(at)
        } else {
            point.world_at(at)
        };
        if let Some(t0) = t0 {
            info!("pick-cost: arm pick {} us", t0.elapsed().as_micros());
        }
        pick
    }
}

/// The pick at `at`, for a trace off the call; with `plate`, a plate hovered is its unit.
fn trace_at(In((at, plate)): In<(Option<Vec2>, bool)>, mut tracer: ArmTracer) -> PressPick {
    tracer.pick_with(at, plate)
}

/// Run a one-shot of the click on `world`, the unit edits it queued applied after.
fn run<M>(world: &mut World, system: impl IntoSystem<(), (), M> + 'static) {
    if let Err(err) = crate::script_calls::run_cached(world, system) {
        debug!("world click: a step did not run: {err}");
    }
}

/// Trace the pick at `at` into [`PressPick`], off the call: an arm's, the world's alone, or with
/// `plate` a plate's press, the plate's unit.
fn latch(world: &mut World, at: Option<Vec2>, plate: bool) {
    match world.run_system_cached_with(trace_at, (at, plate)) {
        Ok(pick) => *world.resource_mut::<PressPick>() = pick,
        Err(err) => debug!("world click: the pick did not run: {err}"),
    }
}

/// Whether the world frame took a press: one no frame took, or any while a look session holds the
/// cursor (in freelook every button goes to the world frame, `0x492b50`).
fn world_press(world: &World, over_ui: bool) -> bool {
    let in_world = world
        .get_resource::<State<crate::client_state::ClientState>>()
        .is_some_and(|s| *s.get() == crate::client_state::ClientState::InWorld);
    let looking = world
        .get_resource::<crate::player::CameraControl>()
        .is_some_and(|rig| rig.is_looking());
    in_world && (looking || !over_ui)
}

/// The world frame's right press (`CGWorldFrame::OnMouseDown 0x483c40`), which
/// [`right_press_hook`] runs when the world frame took it: its hook `0x492c20` at `0x483c62`, ahead
/// of the binding (`0x483c93`), so the binding and the pick its arm takes read what the hook left.
/// The hook's legs in its order: a pending target cancelled (`0x492c26`-`0x492c3f`), a pending
/// gift wrap's cursor cleared (`0x492c44`, `0x492c54`), a Repair base mode back to Point
/// (`0x492c59`-`0x492c72`). It tests no payload and consumes nothing (`0x492c77`), so the binding
/// runs. Then the targeting word as the hook left it goes to the VM, for the binding's
/// `SpellIsTargeting`.
fn right_press(world: &mut World) {
    run(
        world,
        crate::spell::targeting::cancel_targeting_on_right_press,
    );
    run(world, crate::ui_items::clear_wrap_on_right_press);
    run(world, crate::ui_merchant::end_repair_mode_on_right_press);
    run(world, crate::spell::targeting::feed_targeting_to_input);
}

/// A right press, stopped ahead of its binding (`0x483c93`): the world frame's hook
/// ([`right_press`]) when the world frame took it.
pub(crate) fn right_press_hook(world: &mut World, over_ui: bool) {
    if world_press(world, over_ui) {
        right_press(world);
    }
}

/// The frame's records begin: an arm made before them traced no press of theirs.
pub(crate) fn begin_records(world: &mut World) {
    if let Some(mut arm) = world.get_resource_mut::<ClickArm>() {
        arm.latched = false;
    }
}

/// The world's steps of one input record, as its walk returns and before the next record: an
/// arm made with no pick, traced at the record's point, and the steps waiting for it; and a press
/// on a nameplate's pick. The click its mouse commands made applied inside their calls.
pub(crate) fn world_steps(world: &mut World, record: &InputRecord) {
    let press = match record.record {
        Record::Button {
            button,
            state: bevy::input::ButtonState::Pressed,
        } => Some(button),
        _ => None,
    };
    settle(world, record.at);
    let armed = world
        .get_resource_mut::<ClickArm>()
        .is_some_and(|mut arm| std::mem::take(&mut arm.latched));
    if press.is_some() {
        // The plate the walk left hovered, as the record's press found it.
        run(world, crate::vplates::seed_plate_hover);
    }
    // A press a nameplate took: the pick names the plate's unit as the press found it, which the
    // plate's click reads at the release for the cursor's verdict on it (`0x7cb910` → `0x4949f0`).
    let plate = world
        .get_resource::<crate::vplates::PlateHover>()
        .is_some_and(|h| h.0.is_some());
    if matches!(press, Some(MouseButton::Left | MouseButton::Right)) && plate && !armed {
        latch(world, record.at, true);
    }
}

/// Trace an arm that found no pick at `at`, then apply the click steps waiting, through
/// the call host, in the order made; again while an applied arm leaves its steps waiting.
fn settle(world: &mut World, at: Option<Vec2>) {
    // Each pass traces the arm the last one left untraced; the bound is a guard.
    for _ in 0..16 {
        let untraced = world.get_resource_mut::<ClickArm>().is_some_and(|mut arm| {
            arm.latched |= arm.untraced;
            std::mem::take(&mut arm.untraced)
        });
        if untraced {
            latch(world, at, false);
        }
        if !crate::script_calls::submit_waiting_clicks(world) {
            return;
        }
    }
}

/// Where the frame's records end: the click steps made with no host lent or waiting for an arm's
/// pick, applied now (a packet handler's `MouselookStart`, a stuck-latch sweep's release), then the
/// legs for what no call dispatched.
pub(crate) fn run_world_clicks(world: &mut World) {
    settle(world, None);
    world.run_schedule(WorldClickLegs);
}

#[cfg(test)]
#[path = "arm_tests.rs"]
mod tests;
