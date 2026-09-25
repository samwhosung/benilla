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
    let steer_held = binds.pressed(crate::bindings::cmd::MOVE_AND_STEER);
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
        set(
            rig.world_mouse.held(LookButton::Right) || steer_held,
            bit::RIGHT_MOUSE,
        );
        set(
            rig.world_mouse.held(LookButton::Left) || steer_held,
            bit::LEFT_MOUSE,
        );
        // `/follow` sets the forward bit through W's own setter, so it arms the camera like W.
        set(
            binds.pressed(crate::bindings::cmd::MOVE_FORWARD) || player.follow_forward,
            bit::FORWARD,
        );
        set(
            binds.pressed(crate::bindings::cmd::MOVE_BACKWARD),
            bit::BACKWARD,
        );
        set(
            binds.pressed(crate::bindings::cmd::STRAFE_LEFT),
            bit::STRAFE_LEFT,
        );
        set(
            binds.pressed(crate::bindings::cmd::STRAFE_RIGHT),
            bit::STRAFE_RIGHT,
        );
        set(
            binds.pressed(crate::bindings::cmd::TURN_LEFT),
            bit::TURN_LEFT,
        );
        set(
            binds.pressed(crate::bindings::cmd::TURN_RIGHT),
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
    /// Autorun was toggled on this frame, a cast-interrupt term the flag delta cannot see.
    pub autorun_armed: bool,
    /// The raw strafe and turn keys, for the swim amounts and the wire's turn bits.
    pub strafe_left: bool,
    pub strafe_right: bool,
    pub turn_left: bool,
    pub turn_right: bool,
}

/// Decodes the movement keys into [`MoveAxes`], running the autorun latch and its cancel set.
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
    // ── Autorun ── TOGGLEAUTORUN, a latch (1.12 defaults NUMLOCK and BUTTON4, winit's
    // `Forward`). The reference's window-deactivate handler (`0x514490`, called only from the
    // WM_ACTIVATE slot at `0x493058`) releases every direction bit and keeps `0x1000`.
    let mut autorun_armed = false;
    if binds.fired(crate::bindings::cmd::TOGGLE_AUTORUN) {
        player.autorun = !player.autorun;
        autorun_armed = player.autorun;
    }
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
    // - Losing the mover, a level: the emitter gate `0x514560` down (health `<= 0`,
    //   `MOVEMENTFLAGS & 0x1200`, stand state 7) makes writer `0x514748` clear the bit at the next
    //   emit; ours is `state::may_translate`. The server-ride term is benilla's own.
    // A jump, a chat EditBox taking focus and a zone change leave it set; mounting is untraced and
    // leaves it set here. A focused chat box releases nothing, so W held through ENTER still runs.
    let both_buttons_engaged = (both_buttons
        && (rig.world_mouse.down(LookButton::Left) || rig.world_mouse.down(LookButton::Right)))
        || binds.just_pressed(crate::bindings::cmd::MOVE_AND_STEER);
    if state::autorun_cancelled(
        binds.just_pressed(crate::bindings::cmd::MOVE_FORWARD),
        binds.just_pressed(crate::bindings::cmd::MOVE_BACKWARD),
        both_buttons_engaged,
        !may_translate || player.server_riding,
    ) {
        player.autorun = false;
    }
    let autorun = player.autorun;
    // ── The forward/back axis ── The order of S and autorun decides: S held, then autorun
    // toggled, keeps both (the toggle pushes `0x1000`, so `test cl,0x30` misses) and the
    // reference sends MSG_MOVE_STOP with S held; autorun, then S, clears the bit at key-down and
    // walks you backward. `/follow` is a held forward, W's own bit `0x100000` through W's setter,
    // so it nets against S like W and never trips the cancel set, which fires on key-down edges.
    let fwd_axis = state::forward_axis(
        binds.pressed(crate::bindings::cmd::MOVE_FORWARD) || player.follow_forward,
        binds.pressed(crate::bindings::cmd::MOVE_BACKWARD),
        both_buttons,
        autorun,
    );
    // The 1.12 control model (`0x7c5360`): A/D turn the character unless right-mouse is held,
    // when they strafe and the facing tracks the camera; Q/E always strafe. W walks along the
    // character's facing, so a left-drag orbit does not change it.
    let mouselook = both_buttons || rig.look == Some(LookButton::Right);
    // The strafe axis nets like `fwd_axis`: vmangos relays no packet carrying both strafe bits,
    // and the reference's 1.12.1 capture sends none.
    let strafe_left = binds.pressed(crate::bindings::cmd::STRAFE_LEFT);
    let strafe_right = binds.pressed(crate::bindings::cmd::STRAFE_RIGHT);
    let turn_left = binds.pressed(crate::bindings::cmd::TURN_LEFT);
    let turn_right = binds.pressed(crate::bindings::cmd::TURN_RIGHT);
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
        autorun_armed,
        strafe_left,
        strafe_right,
        turn_left,
        turn_right,
    }
}
