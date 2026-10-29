//! What the binding functions called from Lua ([`BindingInput`]) leave for the engine. Every key
//! runs its command's `Bindings.xml` body, so this is the one way a key moves the player: in the
//! reference `MOVEFORWARD` runs `MoveForwardStart()` on the press and `MoveForwardStop()` on the
//! release, and both halves set and clear one bit of `[InputControl+4]` through `0x515090`; so a
//! Start with no Stop keeps moving, a Stop clears the bit whichever input set it, and a one-shot
//! fires once per call.

use benilla_ui::script::{BindingInput, FiredInput, HeldInput};

pub(crate) use benilla_ui::script::ViewAxis;

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
    /// `PitchUpStart`/`Stop`, bit `0x400`.
    PitchUp,
    /// `PitchDownStart`/`Stop`, bit `0x800`.
    PitchDown,
    /// `Jump()`.
    Jump,
    /// `ToggleRun()`, the walk toggle.
    ToggleRun,
    /// `ToggleAutoRun()`.
    ToggleAutoRun,
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
        HeldInput::PitchUp => Input::PitchUp,
        HeldInput::PitchDown => Input::PitchDown,
    }
}

/// The one-shot a fire names, and its amount: a zoom's yards, else 1.0.
fn fired_input(input: FiredInput) -> (Input, f32) {
    match input {
        FiredInput::Jump => (Input::Jump, 1.0),
        FiredInput::ToggleRun => (Input::ToggleRun, 1.0),
        FiredInput::ToggleAutoRun => (Input::ToggleAutoRun, 1.0),
        FiredInput::ToggleSheath => (Input::ToggleSheath, 1.0),
        FiredInput::CameraZoomIn(yards) => (Input::CameraZoomIn, yards),
        FiredInput::CameraZoomOut(yards) => (Input::CameraZoomOut, yards),
    }
}

impl BindingsState {
    /// The binding pass begins: last frame's edges are spent, and the calls made after its
    /// readers ran are this frame's.
    pub(super) fn begin_pass(&mut self) {
        self.edges = std::mem::take(&mut self.carry);
        self.open = true;
    }

    /// The binding pass ends: the readers follow, so a call from here on is the next frame's.
    pub(super) fn end_pass(&mut self) {
        self.open = false;
    }

    /// Apply Lua's binding calls that waited for no host, in call order.
    pub(super) fn apply_script_input(&mut self, inputs: Vec<BindingInput>) {
        for input in inputs {
            self.apply(input, false);
        }
    }

    /// One binding call inside its verb's call: the bit moves now, and its edge goes to this
    /// frame's readers while the binding pass runs, else to the next frame's, whose head would
    /// clear it unread. The edge is marked settled: the cast cancel and the autorun toggle its
    /// `SetCommands` makes ran in the call. Whether the call is a new edge: a Start on a bit not
    /// held, or a one-shot.
    pub(crate) fn call(&mut self, input: BindingInput) -> bool {
        self.apply(input, true)
    }

    fn apply(&mut self, input: BindingInput, settled: bool) -> bool {
        let edges = if settled && !self.open {
            &mut self.carry
        } else {
            &mut self.edges
        };
        match input {
            // The set helper `0x514840` early-outs on a bit already set, so a Start on a held
            // bit is no new edge.
            BindingInput::Start(held) => {
                let i = held_input(held);
                if self.held.contains(&i) {
                    return false;
                }
                self.held.push(i);
                edges.just.push(i);
                if settled {
                    edges.settled.push(i);
                }
                true
            }
            // `0x514b70` clears the bit whoever set it, so a second key still down on the
            // same command stops too, and its release is a no-op second Stop.
            BindingInput::Stop(held) => {
                let i = held_input(held);
                self.held.retain(|&h| h != i);
                false
            }
            // A one-shot is its own press edge.
            BindingInput::Fire(fired) => {
                let (i, amount) = fired_input(fired);
                edges.fired.push(i);
                edges.just.push(i);
                edges.amounts.push((i, amount));
                if settled {
                    edges.settled.push(i);
                }
                true
            }
            // The camera's axes are its own state (`[cam+0x128]`), not `[InputControl+4]`:
            // passed through in call order for the camera to apply.
            BindingInput::ViewStart(axis) => {
                edges.view_calls.push((axis, true));
                false
            }
            BindingInput::ViewStop(axis) => {
                edges.view_calls.push((axis, false));
                false
            }
        }
    }

    /// The state after `earlier` frames' calls and this frame's `now`: the earlier edges are
    /// spent, as the frame start clears them, and the earlier holds stay.
    #[cfg(test)]
    pub(crate) fn after_calls(earlier: Vec<BindingInput>, now: Vec<BindingInput>) -> Self {
        let mut state = Self::default();
        state.apply_script_input(earlier);
        state.edges = Default::default();
        state.apply_script_input(now);
        state
    }
}
