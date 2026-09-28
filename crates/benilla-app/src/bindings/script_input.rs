//! The binding functions called from Lua ([`BindingInput`]), applied to the same state their keys
//! drive. In the reference a movement key's body is its Lua function (`MOVEFORWARD` runs
//! `MoveForwardStart()` on the press and `MoveForwardStop()` on the release), and both halves set
//! and clear one bit of `[InputControl+4]` through `0x515090`; so a Start with no Stop keeps
//! moving, a Stop clears the bit whichever input set it, and a one-shot fires as its key does.

use benilla_ui::script::{BindingInput, FiredInput, HeldInput};

use super::commands::{cmd, Cmd};
use super::{BindingsState, Bound};

/// The held command a Start or Stop names.
fn held_cmd(input: HeldInput) -> Cmd {
    match input {
        HeldInput::MoveForward => cmd::MOVE_FORWARD,
        HeldInput::MoveBackward => cmd::MOVE_BACKWARD,
        HeldInput::TurnLeft => cmd::TURN_LEFT,
        HeldInput::TurnRight => cmd::TURN_RIGHT,
        HeldInput::StrafeLeft => cmd::STRAFE_LEFT,
        HeldInput::StrafeRight => cmd::STRAFE_RIGHT,
    }
}

/// The one-shot command a fire names, and its amount: a zoom's yards, else the key's 1.0 step.
fn fired_cmd(input: FiredInput) -> (Cmd, f32) {
    match input {
        FiredInput::Jump => (cmd::JUMP, 1.0),
        FiredInput::ToggleRun => (cmd::TOGGLE_RUN, 1.0),
        FiredInput::ToggleAutoRun => (cmd::TOGGLE_AUTORUN, 1.0),
        FiredInput::SitOrStand => (cmd::SIT_OR_STAND, 1.0),
        FiredInput::ToggleSheath => (cmd::TOGGLE_SHEATH, 1.0),
        FiredInput::CameraZoomIn(yards) => (cmd::CAMERA_ZOOM_IN, yards),
        FiredInput::CameraZoomOut(yards) => (cmd::CAMERA_ZOOM_OUT, yards),
    }
}

impl BindingsState {
    /// Apply Lua's binding calls, in call order.
    pub(super) fn apply_script_input(&mut self, inputs: Vec<BindingInput>) {
        for input in inputs {
            match input {
                // The set helper `0x514840` early-outs on a bit already set, so a Start on a held
                // command is no new edge.
                BindingInput::Start(held) => {
                    let c = held_cmd(held);
                    if !self.pressed(c) {
                        self.just.push(c);
                        self.script_held.push(c);
                    }
                }
                // `0x514b70` clears the bit whoever set it: a key still down stays latched to
                // nothing, so the release that follows is a no-op, as the reference's second Stop.
                BindingInput::Stop(held) => {
                    let c = held_cmd(held);
                    self.script_held.retain(|&h| h != c);
                    self.latched.retain(|&(_, b)| b != Bound::Spec(c));
                }
                BindingInput::Fire(fired) => {
                    let (c, amount) = fired_cmd(fired);
                    self.fired.push(c);
                    if !self.pressed(c) {
                        self.just.push(c);
                    }
                    self.amounts.push((c, amount));
                }
            }
        }
    }

    /// A key's release of a held command runs its Stop, which clears a Lua Start's bit too.
    pub(super) fn release_script_held(&mut self, c: Cmd) {
        self.script_held.retain(|&h| h != c);
    }
}

#[cfg(test)]
mod tests {
    use super::super::commands::{Kind, SPECS};
    use super::*;

    /// Every name the VM can send is a command of the kind it drives: a Start and Stop a held one,
    /// a fire a host one.
    #[test]
    fn every_script_input_names_a_command_of_its_kind() {
        use HeldInput::*;
        for held in [
            MoveForward,
            MoveBackward,
            TurnLeft,
            TurnRight,
            StrafeLeft,
            StrafeRight,
        ] {
            let spec = &SPECS[held_cmd(held).0 as usize];
            assert!(matches!(spec.kind, Kind::Held), "{held:?} → {}", spec.name);
        }
        for fired in [
            FiredInput::Jump,
            FiredInput::ToggleRun,
            FiredInput::ToggleAutoRun,
            FiredInput::SitOrStand,
            FiredInput::ToggleSheath,
            FiredInput::CameraZoomIn(1.0),
            FiredInput::CameraZoomOut(1.0),
        ] {
            let spec = &SPECS[fired_cmd(fired).0 .0 as usize];
            assert!(matches!(spec.kind, Kind::Host), "{fired:?} → {}", spec.name);
        }
    }
}
