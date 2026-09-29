//! What the binding functions called from Lua ([`BindingInput`]) leave for the engine. Every key
//! runs its command's `Bindings.xml` body, so this is the one way a key moves the player: in the
//! reference `MOVEFORWARD` runs `MoveForwardStart()` on the press and `MoveForwardStop()` on the
//! release, and both halves set and clear one bit of `[InputControl+4]` through `0x515090`; so a
//! Start with no Stop keeps moving, a Stop clears the bit whichever input set it, and a one-shot
//! fires once per call.

use benilla_ui::script::{BindingInput, FiredInput, HeldInput};

use super::BindingsState;

/// One engine input a binding function drives: a held bit of `[InputControl+4]` or a one-shot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Input {
    /// `MoveForwardStart`/`Stop`, bit `0x10`.
    MoveForward,
    /// `MoveBackwardStart`/`Stop`, bit `0x20`.
    MoveBackward,
    /// `TurnLeftStart`/`Stop`, bit `0x100`.
    TurnLeft,
    /// `TurnRightStart`/`Stop`, bit `0x200`.
    TurnRight,
    /// `StrafeLeftStart`/`Stop`, bit `0x40`.
    StrafeLeft,
    /// `StrafeRightStart`/`Stop`, bit `0x80`.
    StrafeRight,
    /// `TurnOrActionStart`/`Stop`, bit `0x1`.
    TurnOrAction,
    /// `CameraOrSelectOrMoveStart`/`Stop`, bit `0x2`.
    CameraOrSelectOrMove,
    /// `Jump()`.
    Jump,
    /// `ToggleRun()`, the walk toggle.
    ToggleRun,
    /// `ToggleAutoRun()`.
    ToggleAutoRun,
    /// `SitOrStand()`.
    SitOrStand,
    /// `ToggleSheath()`.
    ToggleSheath,
    /// `CameraZoomIn([yards])`, its yards in [`BindingsState::amount`].
    CameraZoomIn,
    /// `CameraZoomOut([yards])`, its yards in [`BindingsState::amount`].
    CameraZoomOut,
}

/// The bit a Start or Stop names.
fn held_input(input: HeldInput) -> Input {
    match input {
        HeldInput::MoveForward => Input::MoveForward,
        HeldInput::MoveBackward => Input::MoveBackward,
        HeldInput::TurnLeft => Input::TurnLeft,
        HeldInput::TurnRight => Input::TurnRight,
        HeldInput::StrafeLeft => Input::StrafeLeft,
        HeldInput::StrafeRight => Input::StrafeRight,
        HeldInput::TurnOrAction => Input::TurnOrAction,
        HeldInput::CameraOrSelectOrMove => Input::CameraOrSelectOrMove,
    }
}

/// The one-shot a fire names, and its amount: a zoom's yards, else 1.0.
fn fired_input(input: FiredInput) -> (Input, f32) {
    match input {
        FiredInput::Jump => (Input::Jump, 1.0),
        FiredInput::ToggleRun => (Input::ToggleRun, 1.0),
        FiredInput::ToggleAutoRun => (Input::ToggleAutoRun, 1.0),
        FiredInput::SitOrStand => (Input::SitOrStand, 1.0),
        FiredInput::ToggleSheath => (Input::ToggleSheath, 1.0),
        FiredInput::CameraZoomIn(yards) => (Input::CameraZoomIn, yards),
        FiredInput::CameraZoomOut(yards) => (Input::CameraZoomOut, yards),
    }
}

impl BindingsState {
    /// Apply Lua's binding calls, in call order.
    pub(super) fn apply_script_input(&mut self, inputs: Vec<BindingInput>) {
        for input in inputs {
            match input {
                // The set helper `0x514840` early-outs on a bit already set, so a Start on a held
                // bit is no new edge.
                BindingInput::Start(held) => {
                    let i = held_input(held);
                    if !self.pressed(i) {
                        self.just.push(i);
                        self.held.push(i);
                    }
                }
                // `0x514b70` clears the bit whoever set it, so a second key still down on the
                // same command stops too, and its release is a no-op second Stop.
                BindingInput::Stop(held) => {
                    let i = held_input(held);
                    self.held.retain(|&h| h != i);
                }
                // A one-shot is its own press edge.
                BindingInput::Fire(fired) => {
                    let (i, amount) = fired_input(fired);
                    self.fired.push(i);
                    self.just.push(i);
                    self.amounts.push((i, amount));
                }
            }
        }
    }
}
