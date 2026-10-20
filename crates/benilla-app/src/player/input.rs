//! This frame's decoded input: [`look_input`], the both-button state and the camera command word,
//! and [`move_axes`], the netted axes with the autorun latch. Netting once keeps the direction,
//! the speed, the swim amounts and the streamed flags in agreement.

use bevy::prelude::*;

use super::{camera, state, CameraControl, LookButton, Player};

/// What [`look_input`] read off the mouse and the bindings, before the look session runs.
pub(super) struct LookInput {
    /// The both-button run: both mouse buttons held, or MOVEANDSTEER.
    pub both_buttons: bool,
    /// The camera's input command word ([`super::camera::follow_cmd`]).
    pub follow_command: u32,
}

/// Built before the look session, so the not-driving path, which never reaches [`move_axes`],
/// seats its camera from the same word. The mouse terms come from [`camera::WorldMouse`], never
/// the device: the reference's mouse bits are bindings, so a press the UI took sets neither.
pub(super) fn look_input(
    binds: &crate::bindings::BindingsState,
    player: &Player,
    rig: &CameraControl,
) -> LookInput {
    // Both buttons held run forward and steer with the mouse, whichever went down first.
    // MOVEANDSTEER is the same state: its 1.12 body runs CameraOrSelectOrMove and TurnOrAction.
    let steer_held = binds.steering();
    let both_buttons = rig.world_mouse.both() || steer_held;

    // 1.12's `[InputControl+0x4]`, bit for bit: the camera's auto-follow arms on its edges.
    let follow_command = {
        use camera::follow_cmd as bit;
        let mut w = 0;
        let mut set = |on: bool, b: u32| {
            if on {
                w |= b;
            }
        };
        set(rig.world_mouse.turn() || steer_held, bit::RIGHT_MOUSE);
        set(
            rig.world_mouse.held(LookButton::Left) || steer_held,
            bit::LEFT_MOUSE,
        );
        // The auto-move sets the forward bit through W's own setter, so it arms the camera like W.
        set(
            binds.pressed(crate::bindings::Input::MoveForward) || player.auto_forward,
            bit::FORWARD,
        );
        set(
            binds.pressed(crate::bindings::Input::MoveBackward),
            bit::BACKWARD,
        );
        set(
            binds.pressed(crate::bindings::Input::StrafeLeft),
            bit::STRAFE_LEFT,
        );
        set(
            binds.pressed(crate::bindings::Input::StrafeRight),
            bit::STRAFE_RIGHT,
        );
        set(
            binds.pressed(crate::bindings::Input::TurnLeft),
            bit::TURN_LEFT,
        );
        set(
            binds.pressed(crate::bindings::Input::TurnRight),
            bit::TURN_RIGHT,
        );
        set(player.autorun, bit::AUTORUN);
        // The reference keeps these two in the camera's own Track and Fear bits; here one word
        // carries every edge.
        set(player.server_riding, bit::TRACK);
        set(player.control_lost, bit::FEAR);
        w
    };

    LookInput {
        both_buttons,
        follow_command,
    }
}

/// This frame's netted movement axes and modes; every consumer downstream reads these, not keys.
#[derive(Clone, Copy)]
pub(super) struct MoveAxes {
    /// Net forward/back ([`state::forward_axis`]): its sign is the direction, 0 a cancelled pair.
    pub fwd: i32,
    /// Net strafe, positive right: Q/E always, A/D only while mouse-looking.
    pub side: i32,
    /// Right-mouse (or both-button) held: A/D strafe and the facing tracks the camera.
    pub mouselook: bool,
    /// A keyboard turn is held and allowed (a stun refuses it).
    pub turning: bool,
    /// The reference's `flags & 0xf`, off the net axes: W+S neither streams nor moves.
    pub translating: bool,
    /// The raw strafe and turn keys, for the swim amounts and the wire's turn bits.
    pub strafe_left: bool,
    pub strafe_right: bool,
    pub turn_left: bool,
    pub turn_right: bool,
}

/// `ToggleAutoRun` (`0x513de0`, bound by 1.12 to NUMLOCK and BUTTON4, winit's `Forward`): reads
/// the held bit 0x1000 (`0x513de7`) and presses it when clear, storing it at `0x514907`, or
/// releases it when set (`0x514b70`), whatever drives the body. Whether this frame's press set it:
/// the on edge, a press in the cancel mask. The window-deactivate handler (`0x514490`, called only
/// from the WM_ACTIVATE slot at `0x493058`) releases every direction bit and keeps `0x1000`.
pub(super) fn toggle_autorun(binds: &crate::bindings::BindingsState, player: &mut Player) -> bool {
    if !binds.fired(crate::bindings::Input::ToggleAutoRun) {
        return false;
    }
    player.autorun = !player.autorun;
    player.autorun
}

/// Decodes the movement keys into [`MoveAxes`], running autorun's cancel set on the latch
/// [`toggle_autorun`] has set this frame.
pub(super) fn move_axes(
    binds: &crate::bindings::BindingsState,
    buttons: &ButtonInput<MouseButton>,
    player: &mut Player,
    rig: &CameraControl,
    both_buttons: bool,
    // The reference's input predicates `0x514560` and `0x5145b0` (`state::may_translate`,
    // `state::may_turn`); death takes both down.
    may_translate: bool,
    may_turn: bool,
) -> MoveAxes {
    // Name an extra mouse button the bindings do not know, rather than fail silently.
    for b in buttons.get_just_pressed() {
        if let MouseButton::Other(n) = b {
            info!("mouse: unmapped button Other({n}) — bindings know BUTTON4/BUTTON5 as winit Forward/Back");
        }
    }
    // ── The cancel set ── What clears autorun is what makes it a mode, not a held forward. Of
    // the reference's six writers, these have an analog here:
    // - A W or S key-down, unconditionally: the directional handlers tail into the shared SET
    //   helper `0x514840`, which clears `0x1000` under `test cl,0x30` at `0x514a5a`, before the
    //   axis is built (`0x5150a7`, emitter tail `0x5151a0`). The release path `0x514b70` restores
    //   nothing, so letting go of S after reversing leaves you standing.
    // - The transition into both buttons held (`0x514a73`, the same helper).
    // - The unit unable to translate, a level: the emitter gate `0x514560` down (health `<= 0`,
    //   `MOVEMENTFLAGS & 0x1200`, stand state 7) makes writer `0x514748` clear the bit at the next
    //   emit; ours is `state::may_translate`. A server ride fails it too (`0x5144e0`), and
    //   `control` clears the bit on that path, which never reaches here.
    // A jump, a chat EditBox taking focus and a zone change leave it set; mounting is untraced and
    // leaves it set here. A focused chat box releases nothing, so W held through ENTER still runs.
    let both_buttons_engaged = both_engaged(binds, rig, both_buttons);
    if state::autorun_cancelled(
        binds.just_pressed(crate::bindings::Input::MoveForward),
        binds.just_pressed(crate::bindings::Input::MoveBackward),
        both_buttons_engaged,
        !may_translate,
    ) {
        player.autorun = false;
    }
    let autorun = player.autorun;
    // ── The forward/back axis ── The order of S and autorun decides: S held, then autorun
    // toggled, keeps both (the toggle pushes `0x1000`, so `test cl,0x30` misses) and the
    // reference sends MSG_MOVE_STOP with S held; autorun, then S, clears the bit at key-down and
    // walks you backward. The auto-move is a held forward, W's own bit `0x100000` through W's
    // setter, so it nets against S like W and never trips the cancel set, which fires on key-down
    // edges.
    let fwd_axis = state::forward_axis(
        binds.pressed(crate::bindings::Input::MoveForward) || player.auto_forward,
        binds.pressed(crate::bindings::Input::MoveBackward),
        both_buttons,
        autorun,
    );
    // The 1.12 control model (`0x7c5360`): A/D turn the character unless right-mouse is held,
    // when they strafe and the facing tracks the camera; Q/E always strafe. W walks along the
    // character's facing, so a left-drag orbit does not change it.
    let mouselook = both_buttons || rig.look == Some(LookButton::Right);
    // The strafe axis nets like `fwd_axis`: vmangos relays no packet carrying both strafe bits,
    // and the reference's 1.12.1 capture sends none.
    let strafe_left = binds.pressed(crate::bindings::Input::StrafeLeft);
    let strafe_right = binds.pressed(crate::bindings::Input::StrafeRight);
    let turn_left = binds.pressed(crate::bindings::Input::TurnLeft);
    let turn_right = binds.pressed(crate::bindings::Input::TurnRight);
    let side_axis = i32::from(strafe_right) - i32::from(strafe_left)
        + if mouselook {
            i32::from(turn_right) - i32::from(turn_left)
        } else {
            0
        };
    // A keyboard turn yaws left-positive, like the mouse, and never while `may_turn` is down: the
    // reference skips its turn emitter `0x514f50`, and stops one in flight, behind `0x514755`. A
    // corpse fails the precondition both predicates share (`0x5144e0`), so it cannot turn either.
    let turning = !mouselook && may_turn && (turn_left || turn_right);
    let translating = fwd_axis != 0 || side_axis != 0;

    MoveAxes {
        fwd: fwd_axis,
        side: side_axis,
        mouselook,
        turning,
        translating,
        strafe_left,
        strafe_right,
        turn_left,
        turn_right,
    }
}

/// The transition into both mouse bits held (`0x514a73`, `0x5150e2`): a channel rose with the
/// other held, or MOVEANDSTEER began.
pub(super) fn both_engaged(
    binds: &crate::bindings::BindingsState,
    rig: &CameraControl,
    both_buttons: bool,
) -> bool {
    (both_buttons && rig.world_mouse.rose()) || binds.steering_began()
}

/// Whether this frame's input cancels the cast in flight, as the press handler `0x515090` and
/// `Script::Jump` (`0x513c43`) decide it: a press that newly set forward, back, strafe or autorun
/// (the mask `0x10f0` at `0x5150ce`; a Start on a held bit is no edge, `0x51484c`), the press that
/// completed both mouse bits (`0x5150e2`), or the Jump command, which cancels ahead of its own
/// checks once the active mover resolves (`0x513be0`-`0x513c02`), so not while control is lost
/// (`0x5fa600` zeroes it) or a claimed unit is unstreamed. It reads the commands alone: nothing
/// between the press and the cancel tests root or stun, as the tick `0x514640` that applies them
/// runs after (`0x5151a0`). The turn and pitch bits (`0x100`-`0x800`) never cancel, even while
/// the right button makes a turn key strafe, and neither does follow's forward, which presses
/// `0x100000` through the same handler (`0x60e7ab`), outside the mask and no mouse bit.
pub(super) fn cancels_cast(
    binds: &crate::bindings::BindingsState,
    autorun_on: bool,
    both_engaged: bool,
    has_mover: bool,
) -> bool {
    use crate::bindings::Input;
    [
        Input::MoveForward,
        Input::MoveBackward,
        Input::StrafeLeft,
        Input::StrafeRight,
    ]
    .into_iter()
    .any(|i| binds.just_pressed(i))
        || autorun_on
        || both_engaged
        || has_mover && binds.fired(Input::Jump)
}

#[cfg(test)]
mod tests {
    use benilla_ui::script::{BindingInput, FiredInput, HeldInput};

    use super::*;
    use crate::bindings::BindingsState;

    use BindingInput::{Fire, Start};

    /// One frame's verdict after `earlier`'s calls, with the mover there, and the autorun latch
    /// run as the controller runs it.
    fn cancels(
        earlier: Vec<BindingInput>,
        now: Vec<BindingInput>,
        player: &mut Player,
        rooted: bool,
    ) -> bool {
        let binds = BindingsState::after_calls(earlier, now);
        let rig = CameraControl::default();
        let autorun_on = toggle_autorun(&binds, player);
        let both = both_engaged(&binds, &rig, binds.steering());
        move_axes(
            &binds,
            &ButtonInput::default(),
            player,
            &rig,
            binds.steering(),
            !rooted,
            true,
        );
        cancels_cast(&binds, autorun_on, both, true)
    }

    /// `0x515090` cancels on the command press (`0x5150ce`), and nothing between the press and
    /// the cancel tests root or stun: the tick `0x514640` that applies them runs after
    /// (`0x5151a0`). Under a root the streamed word gains no direction bit.
    #[test]
    fn a_movement_press_while_rooted_cancels_the_cast() {
        for start in [
            HeldInput::MoveForward,
            HeldInput::MoveBackward,
            HeldInput::StrafeLeft,
            HeldInput::StrafeRight,
        ] {
            assert!(
                cancels(vec![], vec![Start(start)], &mut Player::default(), true),
                "{start:?}"
            );
        }
    }

    /// `Script::Jump` cancels before its own checks (`0x513c43`, ahead of `0x513cb1`), so a
    /// rooted or refused jump still cancels.
    #[test]
    fn the_jump_command_cancels_even_when_it_cannot_jump() {
        assert!(cancels(
            vec![],
            vec![Fire(FiredInput::Jump)],
            &mut Player::default(),
            true
        ));
    }

    /// The press tests the command bit, not the net direction: W pressed with S held sets 0x10
    /// anew (`0x514840` returns 1), though the axes cancel to no movement.
    #[test]
    fn a_press_that_nets_to_no_movement_still_cancels() {
        assert!(cancels(
            vec![Start(HeldInput::MoveBackward)],
            vec![Start(HeldInput::MoveForward)],
            &mut Player::default(),
            false
        ));
    }

    /// Turn keys are bits 0x100 and 0x200, outside the mask `0x10f0`, even while the right
    /// button makes them strafe (`0x7c5360`).
    #[test]
    fn a_turn_key_that_strafes_does_not_cancel() {
        let mouse = vec![
            Start(HeldInput::TurnOrAction),
            Start(HeldInput::CameraOrSelectOrMove),
        ];
        assert!(!cancels(
            mouse,
            vec![Start(HeldInput::TurnLeft)],
            &mut Player::default(),
            false
        ));
    }

    /// Follow's forward presses `0x100000` through `0x515090` (`0x60e7ab`), outside the mask and
    /// no mouse bit, so its start cancels nothing.
    #[test]
    fn the_follow_forward_does_not_cancel() {
        let mut player = Player {
            auto_forward: true,
            ..Default::default()
        };
        assert!(!cancels(vec![], vec![], &mut player, false));
    }

    /// A Start on a bit already held is no edge (`0x51484c`), and neither is a release.
    #[test]
    fn a_held_or_released_bit_does_not_cancel() {
        assert!(!cancels(
            vec![Start(HeldInput::MoveForward)],
            vec![Start(HeldInput::MoveForward)],
            &mut Player::default(),
            false
        ));
        assert!(!cancels(
            vec![Start(HeldInput::MoveForward)],
            vec![BindingInput::Stop(HeldInput::MoveForward)],
            &mut Player::default(),
            false
        ));
    }

    /// Autorun's on edge pushes 0x1000 (`ToggleAutoRun` `0x513de0`) and cancels, rooted too;
    /// its off edge is a release. Both buttons coming together cancel (`0x5150e2`).
    #[test]
    fn autorun_on_and_both_buttons_cancel() {
        let toggle = || vec![Fire(FiredInput::ToggleAutoRun)];
        let mut player = Player::default();
        assert!(cancels(vec![], toggle(), &mut player, false));
        assert!(!cancels(vec![], toggle(), &mut player, false));
        assert!(cancels(vec![], toggle(), &mut Player::default(), true));
        assert!(cancels(
            vec![Start(HeldInput::TurnOrAction)],
            vec![Start(HeldInput::CameraOrSelectOrMove)],
            &mut Player::default(),
            true
        ));
    }
}
