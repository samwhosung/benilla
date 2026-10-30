//! A movement command's press or release, among the frame's input records and ahead of its packets:
//! every Lua movement function goes through `CInputControl::SetCommands` (`0x515090`), which sets
//! or clears the bit (`0x514840`, `0x514b70`), cancels the cast for a new direction bit
//! (`0x5150ce`-`0x515175`, sending `CMSG_CANCEL_CAST` at once through `0x6e4940`) and ends by
//! running the movement tick inside the record (`0x5151a0` → `0x514640`). The tick reads root and
//! stun as they stand then (`0x514560`, `0x5145b0`).
//!
//! A binding call's bit change, its autorun bit, its cast cancel and the tick's legs that act at
//! once (the stand-up, the away clear, a start's auto-move stop and loot close, [`TickLegs`]) run
//! inside the call ([`CommandSteps`]), so `MoveForwardStart()` mid-cast has sent
//! `CMSG_CANCEL_CAST` and fired its stop before the script's next line. The tick's start or stop
//! follows the input records here, and is sent there, before the frame's packets
//! ([`benilla_world::schedule::WorldStage::Net`]). The reference's tick only queues it on the mover
//! (`0x617840` → `0x617570`, no send), and the frame's mover update sends it (`0x615b10` →
//! `0x616620` → `0x615c30`) in the frame time, which runs after the frame's packets (`0x420d55`,
//! then `0x420d63`; #2795).
//!
//! Two systems, around the cast bar's self-cancel (`crate::spell::LocalCancel`), which sends the
//! cancel between them: [`press_commands`], the cancel's verdict for the edges no call settled
//! (a call made with no host lent, a world mouse press), then [`press_tick`], the tick.
//! [`super::controller::control`] keeps the frame time: it runs the same axes and flag build after
//! the packets and finds the word this changed already streamed.

use bevy::prelude::*;

use benilla_ui::script::{BindingInput, FiredInput, HeldInput};

use crate::creature_anim::move_flags;
use crate::net::{Embodied, NetCommands};

use super::approach::Approach;
use super::controller::{body_gates, loot_walk_away, BodyGates};
use super::follow::{FollowInput, FollowState};
use super::{flags, input, move_trace, movement_net, posture, state, swim, view_subject};
use super::{Player, SelfPosture};

/// The frame's `SetCommands` calls ([`self`]) up to the tick: the autorun toggle, the cast cancel's
/// verdict, and the auto-move stop the start emitters call first.
#[allow(clippy::type_complexity)]
pub(super) fn press_commands(
    binds: Res<crate::bindings::BindingsState>,
    mut called: ResMut<CallAutorun>,
    mut player: ResMut<Player>,
    // The active mover `0xc4da98`, as whether it is our own character.
    active: Query<Has<crate::net::SelfPlayer>, With<crate::net::ActiveMover>>,
    // The cast bar's self-cancel, which `crate::spell::LocalCancel` sends next.
    mut cancel: ResMut<crate::spell::LocalMoveStart>,
    auto_move: (
        FollowInput,
        ResMut<FollowState>,
        ResMut<Approach>,
        // Where the stop's `AUTOFOLLOW_END` goes, ahead of the frame's packets.
        Option<ResMut<crate::field_watch::NoticeLane>>,
    ),
) {
    if !player.active || player.detached {
        return;
    }
    // The cast bar's self-cancel, at the command press ([`input::cancels_cast`]), ahead of the
    // tick: between the press `0x5150a7` and the cancel `0x51512a` stands only the taint test
    // `0x494a50`, so it runs whoever drives the body, and `0x6e4940` sends `CMSG_CANCEL_CAST` at
    // once (`0x6e49ab`), before the tick's start. It reads the commands, never the streamed word,
    // which a root empties of direction bits. The cast cancel tests no mover (`0x5150f2`); the
    // channel's compares the player with the active mover (`0x51517b`-`0x515191`), which
    // possession sets to the creature and lost control zeroes (`0x5fa600`), so steering a
    // possessed creature, or pressing while feared, ends a cast but not a channel.
    let active_mover = active.iter().next();
    let autorun_on = input::toggle_autorun(&binds, called.0.take(), &mut player);
    // A mouse command's call decided the both-button cancel inside it ([`CommandSteps`]).
    let called = binds.settled(crate::bindings::Input::TurnOrAction)
        || binds.settled(crate::bindings::Input::CameraOrSelectOrMove);
    if input::cancels_cast(
        &binds,
        autorun_on,
        !called && input::both_engaged(&binds),
        active_mover.is_some(),
    ) {
        cancel.pressed = true;
        cancel.mover_is_player = active_mover == Some(true);
    }
    // The start emitters' first call, `0x60e990`, stops an auto-move the input did not start, so
    // the tick's axis nets without `/follow`'s or Click to Move's forward.
    let (input, mut follow, mut approach, mut lane) = auto_move;
    if input.cancels(&player) {
        if super::follow::end(&mut follow, lane.as_deref_mut()) {
            info!("follow: cancelled by the player's own movement input");
        }
        approach.stop();
        player.auto_forward = false;
    }
}

/// The tick a press or release runs at `SetCommands`' tail, when the held command bits changed: the
/// stand-up, then each emitter's start or stop, on the body we drive. A call ran its stand-up and
/// loot close inside it ([`CommandSteps`]); here they find the player standing and the window shut.
#[allow(clippy::type_complexity)]
pub(super) fn press_tick(
    time: Res<Time>,
    binds: Res<crate::bindings::BindingsState>,
    mut player: ResMut<Player>,
    lock: Res<super::MoverLock>,
    net: Res<NetCommands>,
    gates: (
        ResMut<super::StandPending>,
        Res<view_subject::FarSight>,
        Query<Has<Embodied>, With<crate::net::SelfPlayer>>,
        Query<Option<&crate::net::ObjectStore>, With<Embodied>>,
        SelfPosture,
    ),
    // The loot window's move-start close.
    mut loot: ResMut<crate::ui_loot::LootMoveStart>,
    // The auto-move a pitch start or stop ends, and where its `AUTOFOLLOW_END` goes.
    mut auto_move: (
        ResMut<FollowState>,
        ResMut<Approach>,
        Option<ResMut<crate::field_watch::NoticeLane>>,
    ),
    // The pitch latch the frame's calls left, which the emitter's own run takes over.
    call_pitch: Option<ResMut<CallPitch>>,
) {
    let (mut pending, far_sight, self_driven, store, me) = gates;
    let word = input::command_word(&binds);
    let changed = std::mem::replace(&mut player.input_word, word) != word
        || binds.fired(crate::bindings::Input::ToggleAutoRun);
    // `0x514840` and `0x514b70` answer whether a bit changed, and nothing else runs when none did.
    if !player.active || player.detached || !changed {
        return;
    }
    // The tick finds no unit while control is lost or the claimed mover is unstreamed
    // (`0x51466e`), and a server ride fails its precondition (`0x5144e0`): the frame time's
    // not-driving path parks and clears as before. A held mover is the frame time's too: the
    // settle hold streams no locomotion.
    if player.server_riding || lock.control_lost || lock.reseat || player.settling {
        return;
    }
    let BodyGates { stunned, mover, .. } = body_gates(
        store.single().ok().flatten(),
        self_driven.single().unwrap_or(false),
        &pending,
        &lock,
        &far_sight,
    );
    // `0x514560` and `0x5145b0` as the mover stands before this frame's packets.
    let may_translate = mover.may_translate(player.modes.rooted);
    let may_turn = mover.may_turn(stunned);
    let both_buttons = input::look_input(&binds, &player, &lock).both_buttons;
    let axes = input::move_axes(&binds, &mut player, both_buttons, may_translate, may_turn);
    // Translation or a keyboard turn stands a seated body before the start is queued.
    if may_translate && axes.translating || axes.turning {
        posture::stand_up_for_movement(&mut pending, &me, player.move_flags, &net);
    }
    let swimming = player.move_flags & move_flags::SWIMMING != 0;
    let swim = swimming.then(|| swim::translate_amounts(&axes, !may_translate));
    let bits =
        state::incapacitated_flags(flags::command_bits(&axes, swim), !may_translate, !may_turn);
    // The pitch arm, after the turn arm (`0x514777` → `0x515010`): its latch, and its start or stop
    // for the stream to send after the turn's. It runs only while swimming (`0x514766`); stunned,
    // it stops a swimmer's pitch and clears the latch either way (`0x5147e9`, `0x5147fa`).
    let pitched = swim::pitch_emitter(
        &mut player,
        (
            binds.pressed(crate::bindings::Input::PitchUp),
            binds.pressed(crate::bindings::Input::PitchDown),
        ),
        word & super::camera::follow_cmd::RIGHT_MOUSE != 0,
        may_turn,
        swimming,
    );
    if let Some(mut call) = call_pitch {
        call.frame_wrote();
    }
    // A pitch start or stop no call settled ends the auto-move (`0x60ddaa`, `0x60ddda` →
    // `0x60e990(1, 0)`).
    let unsettled = [
        crate::bindings::Input::PitchUp,
        crate::bindings::Input::PitchDown,
    ]
    .iter()
    .all(|&i| !binds.settled(i));
    if pitched && unsettled {
        let (follow, approach, lane) = &mut auto_move;
        super::approach::end_auto_move(Some(follow), Some(approach), None, lane.as_deref_mut());
    }
    let prev = player.move_flags;
    // The tick runs in the air, whose translate predicate tests root alone (`0x514560` tests
    // `0x1200`, not the fall bit), so its legs above ran; the start it queues is the frame time's
    // to stream there, where only the mover's standstill nudge breaks the silence.
    if prev & move_flags::FALLING == 0 {
        let ended = movement_net::stream_commands(&net.0, &mut player, bits, time.elapsed_secs());
        player.press_ended_precast |= ended;
        move_trace::press(word, prev, player.move_flags);
    }
    // A start's `0x60e990` (`0x60dc6a`, `0x60dcba`, `0x60dd55`) closes the loot at the press.
    if lock.foreign_mover.is_none() && (loot_walk_away(prev, bits) || pitched) {
        loot.0 = true;
    }
}

/// The autorun bit `0x1000` as this frame's calls left it, read and written inside each
/// (`0x513de7`, `0x514a5a`, `0x514a73`), which the frame's latch takes at the command press
/// ([`input::toggle_autorun`]); `None` when no call touched it.
#[derive(Resource, Default)]
pub(crate) struct CallAutorun(Option<bool>);

impl CallAutorun {
    /// An auto-move stop's forward release clears `0x1000` inside its handler (`0x514630` at
    /// `0x60e83f`): the calls after it read the bit clear, and the frame's latch takes it.
    pub(crate) fn clear(&mut self) {
        self.0 = Some(false);
    }

    /// The bit as the frame's calls left it, `latched` when none touched it.
    pub(super) fn bit(&self, latched: bool) -> bool {
        self.0.unwrap_or(latched)
    }

    /// What the frame's calls left, for a test.
    #[cfg(test)]
    pub(crate) fn called(&self) -> Option<bool> {
        self.0
    }
}

/// The pitch emitter's latch `0x80000` as the calls since the frame's tick left it, set and
/// cleared inside each (`0x515059`, `0x515078`); `None` when none moved it, where a call reads the
/// frame's, on the player, which the frame's emitter moves as it streams the pitch.
#[derive(Resource, Default)]
pub(crate) struct CallPitch(Option<bool>);

impl CallPitch {
    /// The frame wrote the latch itself, its emitter's run or the mouse hand-off's clear
    /// (`0x515378`): the calls after it read the frame's.
    pub(super) fn frame_wrote(&mut self) {
        self.0 = None;
    }

    /// What the calls left, for a test.
    #[cfg(test)]
    pub(super) fn called(&self) -> Option<bool> {
        self.0
    }

    /// A call's latch, for a test.
    #[cfg(test)]
    pub(super) fn set_for_test(&mut self, latch: bool) {
        self.0 = Some(latch);
    }
}

/// A binding call inside its verb's call ([`benilla_ui::script::ScriptCall::Input`]): the bit
/// set or cleared through `SetCommands` (`0x515090` → `0x514840`, `0x514b70`), for a press that
/// changed a bit the cast cancel's verdict (`0x5150ce`-`0x515193`), then the tick's legs that act
/// at once (`0x5151a0` → `0x514640`, [`TickLegs`]), before the call returns. `ToggleAutoRun`
/// (`0x513de0`) presses or releases `0x1000` by its current state, and `Jump` cancels once the
/// active mover resolves (`0x513be0`-`0x513c43`) and runs its own legs (`0x513d1f`-`0x513d3e`). A
/// one-shot leaves its edge for the frame's readers; the camera axes are the camera's
/// ([`super::camera_view::ViewSteps::move_axis`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CommandSteps<'w, 's> {
    binds: ResMut<'w, crate::bindings::BindingsState>,
    autorun: Option<ResMut<'w, CallAutorun>>,
    pitch_latch: Option<ResMut<'w, CallPitch>>,
    player: Option<Res<'w, Player>>,
    // The active mover `0xc4da98`, as whether it is our own character.
    active: Query<'w, 's, Has<crate::net::SelfPlayer>, With<crate::net::ActiveMover>>,
    tick: TickReads<'w, 's>,
}

/// What the call's tick reads, and the stand-up it sends; each absent in a harness without the
/// mover's state, which runs no tick.
#[derive(bevy::ecs::system::SystemParam)]
struct TickReads<'w, 's> {
    lock: Option<Res<'w, super::MoverLock>>,
    pending: Option<ResMut<'w, super::StandPending>>,
    far_sight: Option<Res<'w, view_subject::FarSight>>,
    net: Option<Res<'w, NetCommands>>,
    self_driven: Query<'w, 's, Has<Embodied>, With<crate::net::SelfPlayer>>,
    store: Query<'w, 's, Option<&'static crate::net::ObjectStore>, With<Embodied>>,
    me: SelfPosture<'w, 's>,
    /// The auto-move a start's `0x60e990` stops (`0x60e9b5`).
    follow: Option<ResMut<'w, FollowState>>,
    approach: Option<ResMut<'w, Approach>>,
}

/// What a binding call leaves its host to run inside it, in the reference's order: the cast
/// cancel, then the tick's legs.
#[derive(Default)]
pub(crate) struct Called {
    /// The cast cancel's verdict: whether the player is the active mover, the one extra test of the
    /// channel's cancel (`0x51517b`-`0x515191`).
    pub(crate) cancel: Option<bool>,
    pub(crate) legs: TickLegs,
}

/// What the tick's emitters make of the command bits as they stand.
#[derive(Clone, Copy)]
struct Emitters {
    axes: input::MoveAxes,
    may_translate: bool,
    /// The direction and turn bits the emitters stream ([`flags::command_bits`]).
    bits: u32,
    /// The pitch emitter's latch after it ran, where it starts or stops a pitch.
    pitch: Option<bool>,
    /// The mover is our own player.
    self_driven: bool,
}

/// The tick's legs that act inside the call, past the stand-up it sent itself: the emitters'
/// away clear for a player on a nonzero total (`0x514e23`, `0x514f0b`, `0x514fca`; Jump's
/// `0x513d36`), then a start's `OnMoveInput` (`0x60e990`, from `0x60dc6a`, `0x60dcba` and the
/// turn starts; Jump's `0x60deae` passes 0 for the auto-move): the auto-move's stop
/// (`0x60e9b5`), then the loot close (`0x60e9dd`). The starts and stops themselves only queue on
/// the mover (`0x617840`), which the frame sends ([`press_tick`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TickLegs {
    pub(crate) clear_afk: bool,
    pub(crate) stop_auto_move: bool,
    pub(crate) close_loot: bool,
}

impl CommandSteps<'_, '_> {
    /// A follow stop's autorun clear, at its place behind `AUTOFOLLOW_END` on the notice lane
    /// ([`crate::field_watch::FieldNotice::AutorunClear`]).
    pub(crate) fn clear_autorun(&mut self) {
        if let Some(autorun) = self.autorun.as_deref_mut() {
            autorun.clear();
        }
    }

    /// Apply `input`, and say what its call runs next ([`Called`]).
    pub(crate) fn call(&mut self, input: BindingInput) -> Called {
        let before = self.emitters(None);
        let released = self.binds.releases(input);
        let changed = self.binds.call(input);
        let Some(player) = self.player.as_deref() else {
            return Called::default();
        };
        if !player.active || player.detached || !(changed || released) {
            return Called::default();
        }
        let cancel = if changed { self.cancel(input) } else { None };
        Called {
            cancel,
            legs: self.tick(input, before),
        }
    }

    /// `MouselookStart`/`Stop`'s `SetCommands` (`0x515090(1, on, …)` at `0x514231`, `0x51425b`) on
    /// the TurnOrAction channel the world mouse holds for it ([`super::LookSteps`]), `changed` when
    /// the channel moved: the press's cancel when it completed both mouse bits, then the tick, as
    /// for the bound command.
    pub(crate) fn mouselook(&mut self, on: bool, changed: bool) -> Called {
        let active = self
            .player
            .as_deref()
            .is_some_and(|p| p.active && !p.detached);
        if !active || !changed {
            return Called::default();
        }
        // The emitters as the bits stood before the call moved TurnOrAction's.
        let before = self.emitters(Some(!on));
        let input = if on {
            BindingInput::Start(HeldInput::TurnOrAction)
        } else {
            BindingInput::Stop(HeldInput::TurnOrAction)
        };
        Called {
            cancel: if on { self.cancel(input) } else { None },
            legs: self.tick(input, before),
        }
    }

    /// Both primary bits held, a `MouselookStart`'s among them.
    fn steering(&self) -> bool {
        self.binds.steering()
    }

    /// For a press that changed a bit, the cancel's verdict, the autorun bit moved as the press
    /// moves it.
    fn cancel(&mut self, input: BindingInput) -> Option<bool> {
        let player = self.player.as_deref()?;
        let mover = self.active.iter().next();
        let both = self.steering();
        // `0x1000` as the frame's calls left it, the latch's until one touches it; with no store
        // for it (a harness), a call's write is dropped.
        let mut scratch = player.autorun;
        let autorun = match self.autorun.as_deref_mut() {
            Some(CallAutorun(bit)) => bit.get_or_insert(player.autorun),
            None => &mut scratch,
        };
        let cancels = match input {
            // The mask `0x10f0` (`0x5150ce`): forward, back and the strafes; the turns and the
            // pitches (`0x100`-`0x800`) never cancel. A forward or back press clears `0x1000`
            // inside it (`test cl,0x30` at `0x514a5a`).
            BindingInput::Start(HeldInput::MoveForward | HeldInput::MoveBackward) => {
                *autorun = false;
                true
            }
            BindingInput::Start(HeldInput::StrafeLeft | HeldInput::StrafeRight) => true,
            // The press that completed both mouse bits (`0x5150d6`-`0x5150ec`), which clears
            // `0x1000` too (`0x514a73`).
            BindingInput::Start(HeldInput::TurnOrAction | HeldInput::CameraOrSelectOrMove) => {
                if both {
                    *autorun = false;
                }
                both
            }
            // `0x1000` read inside the call (`0x513de7`): pressed when clear, which cancels, and
            // released when set, which does not. The frame's latch takes the bit the calls left
            // ([`input::toggle_autorun`]).
            BindingInput::Fire(FiredInput::ToggleAutoRun) => {
                *autorun = !*autorun;
                *autorun
            }
            BindingInput::Fire(FiredInput::Jump) => mover.is_some(),
            _ => false,
        };
        cancels.then_some(mover == Some(true))
    }

    /// What the emitters make of the bits as they stand ([`Emitters`]), the TurnOrAction bit as
    /// `turn_or_action` says when given; `None` where the tick finds no body it drives: our player
    /// not in control, riding a server path, or settling.
    fn emitters(&self, turn_or_action: Option<bool>) -> Option<Emitters> {
        let tick = &self.tick;
        let (Some(player), Some(lock), Some(pending), Some(far_sight)) = (
            self.player.as_deref(),
            tick.lock.as_deref(),
            tick.pending.as_deref(),
            tick.far_sight.as_deref(),
        ) else {
            return None;
        };
        if !player.active
            || player.detached
            || player.server_riding
            || lock.control_lost
            || lock.reseat
            || player.settling
        {
            return None;
        }
        let self_driven = tick.self_driven.single().unwrap_or(false);
        let BodyGates { stunned, mover, .. } = body_gates(
            tick.store.single().ok().flatten(),
            self_driven,
            pending,
            lock,
            far_sight,
        );
        let may_translate = mover.may_translate(player.modes.rooted);
        let may_turn = mover.may_turn(stunned);
        // The emitters' totals over the bits the calls left, autorun's among them, without the
        // auto-move's forward, which no emitter counts (`0x514da0`).
        let mut autorun = self
            .autorun
            .as_deref()
            .map_or(player.autorun, |a| a.bit(player.autorun));
        use crate::bindings::Input;
        let turn_or_action =
            turn_or_action.unwrap_or_else(|| self.binds.pressed(Input::TurnOrAction));
        let both_buttons = turn_or_action && self.binds.pressed(Input::CameraOrSelectOrMove);
        let axes = input::move_axes_with(
            &self.binds,
            &mut autorun,
            false,
            turn_or_action,
            both_buttons,
            may_translate,
            may_turn,
        );
        let swimming = player.move_flags & move_flags::SWIMMING != 0;
        let swim = swimming.then(|| swim::translate_amounts(&axes, !may_translate));
        let bits =
            state::incapacitated_flags(flags::command_bits(&axes, swim), !may_translate, !may_turn);
        // The pitch emitter against its latch (`0x515010`), which runs only while swimming and
        // free to turn (`0x514766`, `0x514755`) and not under the TurnOrAction bit (`0x515019`): a
        // nonzero net with none latched starts a pitch, a zero net stops a latched one; a body
        // that may not turn stops a latched one while swimming (`0x5147ca`-`0x5147fa`).
        let latch = self
            .pitch_latch
            .as_deref()
            .and_then(|c| c.0)
            .unwrap_or(player.pitch_latch);
        let net = self.binds.pressed(Input::PitchUp) != self.binds.pressed(Input::PitchDown);
        let pitch = if !may_turn {
            (latch && swimming).then_some(false)
        } else {
            let runs = swimming && !turn_or_action;
            (runs && net != latch).then_some(net)
        };
        Some(Emitters {
            axes,
            may_translate,
            bits,
            pitch,
            self_driven,
        })
    }

    /// The tick a changed command runs at `SetCommands`' tail (`0x5151a0` → `0x514640`), or
    /// `Jump`'s own legs, over the bits as the call left them against `before`, the emitters as
    /// they stood: the stand-up sent now, the rest returned for the host. A falling body ticks too:
    /// the translate predicate tests `0x1200` alone (`0x514560`).
    fn tick(&mut self, input: BindingInput, before: Option<Emitters>) -> TickLegs {
        use BindingInput::{Fire, Start};
        let jump = matches!(input, Fire(FiredInput::Jump));
        let commands = matches!(
            input,
            Start(_) | BindingInput::Stop(_) | Fire(FiredInput::ToggleAutoRun)
        );
        if !(jump || commands) {
            return TickLegs::default();
        }
        let (Some(now), Some(before)) = (self.emitters(None), before) else {
            return TickLegs::default();
        };
        let (Some(player), Some(pending), Some(net), Some(lock)) = (
            self.player.as_deref(),
            self.tick.pending.as_deref_mut(),
            self.tick.net.as_deref(),
            self.tick.lock.as_deref(),
        ) else {
            return TickLegs::default();
        };
        // A start's `OnMoveInput` (`0x60e990`): a direction or turn bit the emitters start
        // (`0x60dc6a`, `0x60dcba`, `0x60dd55`), a release among them (`0x514e6e`), or the pitch
        // emitter's start or stop (`0x60ddaa`, `0x60ddda`), as its latch stands: a release of
        // the TurnOrAction bit lets a held pitch key start one. `Jump` runs no tick.
        let pitched = !jump && now.pitch.is_some();
        if let (true, Some(latch), Some(call)) =
            (pitched, now.pitch, self.pitch_latch.as_deref_mut())
        {
            call.0 = Some(latch);
        }
        let started = loot_walk_away(before.bits, now.bits) || pitched;
        // `Jump` past its gates (`0x513cb1`-`0x513d05`), the translate predicate's terms; the
        // tick's emitters on a nonzero total, or a keyboard turn the turn emitter runs.
        let moves = if jump {
            now.may_translate
        } else {
            now.may_translate && now.axes.translating || now.axes.turning
        };
        if moves {
            // `0x60be30(0)` (`0x514e0c`, `0x514ee6`, `0x513d1f`), sent now.
            posture::stand_up_for_movement(pending, &self.tick.me, player.move_flags, net);
        }
        let ours = lock.foreign_mover.is_none();
        TickLegs {
            // The type test `0x514f02` (`0x513d2a` for Jump): a player.
            clear_afk: moves && now.self_driven,
            stop_auto_move: !jump && started,
            close_loot: ours && (jump && moves || started),
        }
    }

    /// Whether a follow is in flight, whose `AUTOFOLLOW_END` (`0x60fb84`) its host signals ahead
    /// of [`Self::stop_auto_move`].
    pub(crate) fn following(&self) -> bool {
        self.tick
            .follow
            .as_deref()
            .is_some_and(|f| f.guid.is_some())
    }

    /// A start's `StopAutoMove` (`0x60e9b5` → `0x60fb60`) past its signal: the follow ends
    /// (`0x60fc1e`) and its release clears autorun (`0x60fc08` → `0x514630`), inside the call
    /// ([`super::approach::end_auto_move`]).
    pub(crate) fn stop_auto_move(&mut self) {
        let tick = &mut self.tick;
        super::approach::end_auto_move(
            tick.follow.as_deref_mut(),
            tick.approach.as_deref_mut(),
            self.autorun.as_deref_mut(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::bindings::BindingsState;
    use BindingInput::{Fire, Start};

    /// A world with the input state and our player in control, autorun on or off.
    fn world(autorun: bool) -> World {
        let mut world = World::new();
        world.init_resource::<BindingsState>();
        world.init_resource::<CallAutorun>();
        world.init_resource::<CallPitch>();
        world.init_resource::<Player>();
        let mut player = world.resource_mut::<Player>();
        player.active = true;
        player.autorun = autorun;
        world
    }

    /// Each call inside its verb's call: whether it cancels the cast.
    fn calls(world: &mut World, inputs: Vec<BindingInput>) -> Vec<bool> {
        world
            .run_system_once(move |mut steps: CommandSteps| {
                inputs
                    .iter()
                    .map(|&i| steps.call(i).cancel.is_some())
                    .collect::<Vec<_>>()
            })
            .unwrap()
    }

    /// The frame's latch after the calls: the command press, then the tick's axes.
    fn latched(world: &mut World) -> bool {
        let called = world.resource_mut::<CallAutorun>().0.take();
        world.resource_scope(|world, binds: Mut<BindingsState>| {
            let mut player = world.resource_mut::<Player>();
            input::toggle_autorun(&binds, called, &mut player);
            input::move_axes(&binds, &mut player, false, true, true);
            player.autorun
        })
    }

    /// `ToggleAutoRun` reads `0x1000` inside its call (`0x513de7`): the first of two in one frame
    /// presses it, which cancels, and the second finds it set and releases it, which does not.
    #[test]
    fn a_second_autorun_toggle_in_a_frame_is_a_release() {
        let mut w = world(false);
        let toggle = Fire(FiredInput::ToggleAutoRun);
        assert_eq!(calls(&mut w, vec![toggle, toggle]), [true, false]);
        assert!(!latched(&mut w), "pressed then released");
    }

    /// W clears `0x1000` inside its press (`0x514a5a`), so a toggle after it in the same frame
    /// finds the bit clear and presses it, cancelling, and autorun stays on.
    #[test]
    fn a_toggle_after_forward_presses_autorun_and_cancels() {
        let mut w = world(true);
        let calls = calls(
            &mut w,
            vec![
                Start(HeldInput::MoveForward),
                Fire(FiredInput::ToggleAutoRun),
            ],
        );
        assert_eq!(calls, [true, true]);
        assert!(latched(&mut w), "the toggle came after the clear");
    }
}
