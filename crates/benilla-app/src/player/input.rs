//! **This frame's decoded input**, in one place — the two things the controller derives from the
//! keyboard, the mouse and the binding table before any of it means anything to the avatar:
//!
//! - [`look_input`] — the both-button state and the reference's own **camera command word**
//!   (1.12's `[InputControl+0x4]`), which the auto-follow is armed by. Read
//!   *before* the look session, because both camera seats want it — including the one on the
//!   not-driving path, which returns before the axes below are ever computed. Its two mouse
//!   terms come off [`camera::WorldMouse`], never `ButtonInput`: the reference's mouse bits are
//!   two *bindings* the UI can eat, and reading the device instead swung the camera on every
//!   right-click of a UI row (ledger B364).
//! - [`move_axes`] — the **netted** forward/back and strafe axes plus the modes that select
//!   between them (mouselook, keyboard turn), and the autorun latch with its cancel set. Netted
//!   once here so that the direction we move, the speed we pick, the swim amounts and the flags we
//!   stream can never disagree.
//!
//! Nothing here touches the avatar: `move_axes` writes exactly one field ([`Player::autorun`]),
//! because the autorun latch *is* input state.

use bevy::prelude::*;

use super::{camera, state, CameraControl, LookButton, Player};

/// What [`look_input`] read off the mouse and the bindings, before the look session runs.
pub(super) struct LookInput {
    /// Vanilla's "both-button run" state (either real button pair, or MOVEANDSTEER).
    pub both_buttons: bool,
    /// The camera's input command word — see [`super::camera::follow_cmd`].
    pub follow_command: u32,
}

/// The mouse/binding state the camera needs, built before the look session so the not-driving
/// path (which returns without ever reaching [`move_axes`]) can seat its camera from the same word.
///
/// Takes the rig for one field only — [`camera::WorldMouse`], already latched this frame by
/// [`camera::latch_world_mouse`]. There is deliberately no `ButtonInput` parameter left: every
/// mouse fact here is one the UI gets first refusal on.
pub(super) fn look_input(
    binds: &crate::bindings::BindingsState,
    player: &Player,
    rig: &CameraControl,
) -> LookInput {
    // Both mouse buttons held together = vanilla's "both-button run": the avatar runs forward while
    // the character steers with the mouse (turns like a right-drag), regardless of which button went
    // down first. Off the world-owned pair, not the device's: both primaries pressed over a bag are
    // two clicks the UI took, and the reference's run is its two *bindings* held, neither of which
    // a captured press dispatches.
    // MOVEANDSTEER (default Middle Mouse) is the same state through a binding — 1.12's own body
    // runs the identical CameraOrSelectOrMove + TurnOrAction pair a both-button press does.
    let steer_held = binds.pressed(crate::bindings::cmd::MOVE_AND_STEER);
    let both_buttons = rig.world_mouse.both() || steer_held;

    // The camera's **input command word** — 1.12's `[InputControl+0x4]`, bit for
    // bit. The auto-follow is armed by *edges on this word* and its state is classified from it, so
    // it is built once here, from the same binding state the movement code reads, and handed
    // to both camera seats. MOVEANDSTEER sets both mouse bits because that is what the reference's
    // binding does — it runs the identical CameraOrSelectOrMove + TurnOrAction pair.
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
        // `/follow` is the forward bit in the reference too — the same setter the W key drives,
        // so it arms the camera exactly like a held W.
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
        // The two externally-driven flags, which the reference folds into the camera's own
        // Track/Fear bits rather than the input word — carried here so one word carries every edge.
        set(player.server_riding, bit::TRACK);
        set(player.control_lost, bit::FEAR);
        w
    };

    LookInput {
        both_buttons,
        follow_command,
    }
}

/// This frame's **netted** movement axes and the modes that shape them. Every forward/back,
/// strafe and turn consumer downstream reads these fields and not the keys.
#[derive(Clone, Copy)]
pub(super) struct MoveAxes {
    /// Net forward/back: `+1` forward, `-1` back, `0` for a cancelled pair (see
    /// [`state::forward_axis`]).
    pub fwd: i32,
    /// Net strafe, `+` right — Q/E always, A/D only while mouse-looking.
    pub side: i32,
    /// Right-mouse (or both-button) held: A/D strafe and the facing tracks the camera.
    pub mouselook: bool,
    /// A keyboard turn is being held — and is allowed (a stun kills it).
    pub turning: bool,
    /// The reference's `flags & 0xf`, off the *net* axes: W+S neither streams nor moves.
    pub translating: bool,
    /// Autorun was toggled **on** this frame — 0445's fifth cast-interrupt term, which the
    /// flag delta cannot see.
    pub autorun_armed: bool,
    /// The raw strafe/turn keys, for the consumers that need the pressed state rather than the
    /// net axis: the swim amounts and the wire's turn bits.
    pub strafe_left: bool,
    pub strafe_right: bool,
    pub turn_left: bool,
    pub turn_right: bool,
}

/// Decode the movement keys into [`MoveAxes`], running the autorun latch and its cancel set on
/// the way through.
pub(super) fn move_axes(
    binds: &crate::bindings::BindingsState,
    buttons: &ButtonInput<MouseButton>,
    player: &mut Player,
    rig: &CameraControl,
    both_buttons: bool,
    // The reference's two movement-input predicates this frame ([`state::may_translate`],
    // [`state::may_turn`]) — `0x514560` and `0x5145b0`. Both go down on death.
    may_translate: bool,
    may_turn: bool,
) -> MoveAxes {
    // ── Autorun ── TOGGLEAUTORUN through the binding table (1.12 defaults NUMLOCK +
    // BUTTON4 — the latter is winit's `Forward`, the thumb button this toggle lived on before
    // the table existed, kept by the codec's BUTTON4 mapping). A latched mode, not a held key:
    // the keyboard chord is typing-gated at dispatch like every binding, the mouse chord is
    // not — and the reference agrees, its **OS window-deactivate** handler releasing every
    // direction bit while preserving `0x1000` (`0x514490`'s `and eax,0xfffff00f`, VERIFIED;
    // "window-deactivate" and not "focus-loss", which read as *UI* focus and cost us 2196 —
    // its sole caller `0x493058` hangs off the WM_ACTIVATE callback slot).
    let mut autorun_armed = false;
    if binds.fired(crate::bindings::cmd::TOGGLE_AUTORUN) {
        player.autorun = !player.autorun;
        autorun_armed = player.autorun;
    }
    // A mouse whose extra buttons don't land on Back/Forward would otherwise fail silently, and
    // "nothing happened" is the least debuggable report there is. Name what did arrive.
    for b in buttons.get_just_pressed() {
        if let MouseButton::Other(n) = b {
            info!("mouse: unmapped button Other({n}) — bindings know BUTTON4/BUTTON5 as winit Forward/Back");
        }
    }
    // ── The cancel set ── autorun is NOT simply "held forward" — the thing that makes it its own
    // mode is what *destroys* it. Six writers clear the bit in the reference; these are the ones
    // with a benilla analog:
    //
    // - **A W or S key-DOWN** — unconditional, and the subtle one: the directional handlers look
    //   pure (each pushes only its own bit), but they tail into the shared SET helper `0x514840`,
    //   which does `and [MOVE+4],0xffffefff` under `test cl,0x30` (fwd `0x10` | back `0x20`) at
    //   `0x514a5a`. A per-handler read answers "no" and is wrong about the behaviour. It runs
    //   *before* the axis (`0x5150a7` vs the emitter tail `0x5151a0`), so the axis never sees the
    //   combination. **Key-DOWN only**: the release path `0x514b70` restores nothing, which is why
    //   letting go of S after reversing leaves you standing rather than running again.
    // - **The transition INTO both-buttons-held** (`0x514a73`, the same helper) — engaging the
    //   both-button run replaces autorun rather than stacking with it.
    // - **Losing the mover** — death, or a root/stun. In the reference the emitter's gate
    //   `0x514560` goes down (health `<= 0`, `MOVEMENTFLAGS & 0x1200`, stand state 7) and writer #4
    //   `0x514748` clears the bit as a side effect of the next emit; a level test is the faithful
    //   shape, not an edge. The `death` half of that sentence was prose only until decision 1753:
    //   the term passed here was the root alone, so dying with autorun latched kept the bit. It is
    //   [`state::may_translate`] now — the gate itself.
    //
    //   **This list used to say "a taxi/charge hand-off", on a reading of `0x60f5b0` as an
    //   on-taxi predicate. It is not** — it reads `AnimationData.dbc` column 3 bit `0x80`, set
    //   on exactly one of 208 shipped rows, id 121 `Knockdown`. The ride term below is benilla's
    //   own and is kept on its own merits.
    //
    // Deliberately absent, each VERIFIED as a *survivor*: a jump, a chat EditBox taking focus, and
    // a zone change. Mounting is genuinely unsettled in the reference and left alone here.
    //
    // A chat box taking focus survives *more* than autorun, note: it releases nothing at all —
    // the movement handlers simply become no-ops and the direction bits are frozen (2196), which
    // is why holding W through an ENTER keeps you running.
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
    // ── The forward/back axis ── one net value ([`state::forward_axis`], whose tests pin the
    // verified state table) read by every forward/back consumer downstream, so the direction we move,
    // the speed we pick, the swim amounts and the flags we stream can't disagree.
    //
    // Zero is the state no "autorun = held forward" reading can produce, and it is reachable:
    // hold S *first*, then toggle autorun — the toggle pushes X=`0x1000`, so `test cl,0x30` misses
    // and the bit survives — and the client emits MSG_MOVE_STOP with S still held. The other order
    // (autorun, then S) destroys the bit at key-down and walks you backward. Same two keys, two
    // outcomes; that asymmetry is the whole shape of the feature.
    // `/follow` enters as the FORWARD term, not a fifth source: the reference's
    // follow pushes the very same move-forward bit `0x100000` the W key does, through the same
    // setter, so it nets against a held S and diagonals with a strafe exactly like a held W. It
    // rides the HELD state and not the key-DOWN edge, so it never trips the autorun cancel set
    // above — which is right: synthesized input is not a keypress.
    let fwd_axis = state::forward_axis(
        binds.pressed(crate::bindings::cmd::MOVE_FORWARD) || player.follow_forward,
        binds.pressed(crate::bindings::cmd::MOVE_BACKWARD),
        both_buttons,
        autorun,
    );
    // Vanilla turn/strafe control model (`0x7c5360`): W/S move
    // forward/back in the facing; **A/D turn the character** (rotate the facing at the turn rate) so
    // the body faces where it runs — UNLESS right-mouse is held (mouse-look), where A/D strafe and
    // the facing tracks the camera; **Q/E always strafe**. Movement basis is the *character* facing,
    // so left-drag (camera-only orbit) doesn't change which way W walks.
    let mouselook = both_buttons || rig.look == Some(LookButton::Right);
    // The strafe axis, **netted exactly like `fwd_axis`** — Q/E always strafe, A/D only while
    // mouse-looking. Netting is not a nicety: the two bits are mutually exclusive on the wire.
    // Holding both keys used to OR `STRAFE_LEFT | STRAFE_RIGHT` into the flags while the avatar
    // stood still (the controller's `dir` sum cancels), and vmangos **silently drops** every movement
    // packet carrying that pair — never relaying it to anyone. Measured: 48 such
    // packets in one session, 0 received by a watching client, against 0 in the reference
    // client's entire 1.12.1 capture. That is decision 0056's invariant — the wire mirrors the
    // avatar's actual motion — violated on this one axis only; the swim branch already nets.
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
    // TURNLEFT/TURNRIGHT turn the facing when not mouse-looking (yaw increases turning left,
    // matching mouse-left). …and never while [`state::may_turn`] is down: the reference skips the
    // keyboard turn emitter `0x514f50` entirely (and force-stops an in-flight turn) behind the
    // `0x514755` gate. Killing the turn here is also what ends B179's *second* half — the walk
    // animation a stunned character was still playing was the turn-in-place shuffle, which
    // `gait` derives from real yaw change. A **dead** body is down the same gate, through the
    // precondition `0x5144e0` that both predicates share: health `<= 0` fails it,
    // so as far as `0x5145b0` is concerned a corpse is stunned — which is why our corpses could
    // be spun with A/D until 1753, and why one term fixes the keys and the mouse together.
    let turning = !mouselook && may_turn && (turn_left || turn_right);
    // The net translate state — the reference's `flags & 0xf` (its four move bits), read off
    // the *net* axes, not the keys: W+S streams no direction bit and doesn't translate.
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
