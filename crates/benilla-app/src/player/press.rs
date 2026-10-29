//! A movement command's press or release, among the frame's input records and ahead of its packets:
//! every Lua movement function goes through `CInputControl::SetCommands` (`0x515090`), which sets
//! or clears the bit (`0x514840`, `0x514b70`), cancels the cast for a new direction bit
//! (`0x5150ce`-`0x515175`, sending `CMSG_CANCEL_CAST` at once through `0x6e4940`) and ends by
//! running the movement tick inside the record (`0x5151a0` → `0x514640`). The tick reads root and
//! stun as they stand then (`0x514560`, `0x5145b0`).
//!
//! A binding call's bit change, its autorun bit and its cast cancel run inside the call
//! ([`CommandSteps`]), so `MoveForwardStart()` mid-cast has sent `CMSG_CANCEL_CAST` and fired its
//! stop before the script's next line. The tick follows the input records here, and its start or
//! stop is sent there, before the frame's packets
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
use super::{CameraControl, Player, SelfPosture};

/// The frame's `SetCommands` calls ([`self`]) up to the tick: the autorun toggle, the cast cancel's
/// verdict, and the auto-move stop the start emitters call first.
#[allow(clippy::type_complexity)]
pub(super) fn press_commands(
    binds: Res<crate::bindings::BindingsState>,
    mut called: ResMut<CallAutorun>,
    rig: Res<CameraControl>,
    mut player: ResMut<Player>,
    lock: Res<super::MoverLock>,
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
    let both_buttons = input::look_input(&binds, &player, &lock, &rig).both_buttons;
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
        !called && input::both_engaged(&binds, &rig, both_buttons),
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
/// stand-up, then each emitter's start or stop, on the body we drive.
#[allow(clippy::type_complexity)]
pub(super) fn press_tick(
    time: Res<Time>,
    binds: Res<crate::bindings::BindingsState>,
    rig: Res<CameraControl>,
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
) {
    let (mut pending, far_sight, self_driven, store, me) = gates;
    let word = input::command_word(&binds, &rig);
    let changed = std::mem::replace(&mut player.input_word, word) != word
        || binds.fired(crate::bindings::Input::ToggleAutoRun);
    // `0x514840` and `0x514b70` answer whether a bit changed, and nothing else runs when none did.
    if !player.active || player.detached || !changed {
        return;
    }
    // The tick finds no unit while control is lost or the claimed mover is unstreamed
    // (`0x51466e`), and a server ride fails its precondition (`0x5144e0`): the frame time's
    // not-driving path parks and clears as before. A held or falling mover is the frame time's
    // too: the settle hold streams no locomotion, and in the air only the mover's standstill nudge
    // breaks the silence, which its step decides.
    if player.server_riding
        || lock.control_lost
        || lock.reseat
        || player.settling
        || player.move_flags & move_flags::FALLING != 0
    {
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
    let both_buttons = input::look_input(&binds, &player, &lock, &rig).both_buttons;
    let axes = input::move_axes(
        &binds,
        &mut player,
        &rig,
        both_buttons,
        may_translate,
        may_turn,
    );
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
    swim::pitch_emitter(
        &mut player,
        (
            binds.pressed(crate::bindings::Input::PitchUp),
            binds.pressed(crate::bindings::Input::PitchDown),
        ),
        word & super::camera::follow_cmd::RIGHT_MOUSE != 0,
        may_turn,
        swimming,
    );
    let prev = player.move_flags;
    let ended = movement_net::stream_commands(&net.0, &mut player, bits, time.elapsed_secs());
    player.press_ended_precast |= ended;
    move_trace::press(word, prev, player.move_flags);
    if lock.foreign_mover.is_none() && loot_walk_away(prev, player.move_flags) {
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

/// A binding call inside its verb's call ([`benilla_ui::script::ScriptCall::Input`]): the bit
/// set or cleared through `SetCommands` (`0x515090` → `0x514840`, `0x514b70`), and for a press
/// that changed a bit the cast cancel's verdict (`0x5150ce`-`0x515193`), before the call returns.
/// `ToggleAutoRun` (`0x513de0`) presses or releases `0x1000` by its current state, and `Jump`
/// cancels once the active mover resolves (`0x513be0`-`0x513c43`). A one-shot and the camera axes
/// leave their edge for the frame's readers.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CommandSteps<'w, 's> {
    binds: ResMut<'w, crate::bindings::BindingsState>,
    autorun: Option<ResMut<'w, CallAutorun>>,
    player: Option<Res<'w, Player>>,
    // The active mover `0xc4da98`, as whether it is our own character.
    active: Query<'w, 's, Has<crate::net::SelfPlayer>, With<crate::net::ActiveMover>>,
}

impl CommandSteps<'_, '_> {
    /// A follow stop's autorun clear, at its place behind `AUTOFOLLOW_END` on the notice lane
    /// ([`crate::field_watch::FieldNotice::AutorunClear`]).
    pub(crate) fn clear_autorun(&mut self) {
        if let Some(autorun) = self.autorun.as_deref_mut() {
            autorun.clear();
        }
    }

    /// Apply `input`; when it cancels the cast, whether the player is the active mover, the one
    /// extra test of the channel's cancel (`0x51517b`-`0x515191`).
    pub(crate) fn call(&mut self, input: BindingInput) -> Option<bool> {
        let changed = self.binds.call(input);
        let player = self.player.as_deref()?;
        if !player.active || player.detached || !changed {
            return None;
        }
        let mover = self.active.iter().next();
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
                let both = self.binds.steering();
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
                    .map(|&i| steps.call(i).is_some())
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
            input::move_axes(
                &binds,
                &mut player,
                &CameraControl::default(),
                false,
                true,
                true,
            );
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
