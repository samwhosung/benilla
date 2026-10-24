//! The functions the stock `Bindings.xml` bodies call, from the reference's three registrar
//! tables: the movement table `0x8500b8` (loop `0x514349`), the camera table `0x84f7a0` (loop
//! `0x50c089`) and the targeting and posture verbs of the main table `0x83de68` (loop
//! `0x49027c`). A key and its Lua function share one path: a held command's Start and Stop are the
//! key's press and release ([`HeldInput`]), a one-shot command fires as its key does
//! ([`FiredInput`]), and the targeting and attack calls are script calls ([`ScriptCall`]),
//! applied inside the verb, so a macro's `TargetNearestEnemy()` then `/cast` casts at the new
//! target.
//!
//! Every movement function but the mouselook three opens on the protected-action gate `0x494a50`
//! with tag 0 ([`protected`]), which is all twenty-three of its tag-0 callers. The four
//! `TargetNearest*` shims reach it with tag 4 (`0x493f9b`): a call it refuses picks nothing.
//! The camera table and the other main-table verbs have no gate.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag, flag_or_default};
use super::calls::{NearestMode, ScriptCall};
use super::protected::{self, Event, Tag};
use super::{Model, UiScript};

/// A held movement command: one bit of the reference's `[InputControl+4]` word, set by its Start
/// and cleared by its Stop through `0x515090`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldInput {
    /// `0x10`, `MoveForwardStart`/`Stop` (`0x513e20`/`0x513e50`).
    MoveForward,
    /// `0x20`, `MoveBackwardStart`/`Stop` (`0x513e80`/`0x513eb0`).
    MoveBackward,
    /// `0x100`, `TurnLeftStart`/`Stop` (`0x513ee0`/`0x513f10`).
    TurnLeft,
    /// `0x200`, `TurnRightStart`/`Stop` (`0x513f40`/`0x513f70`).
    TurnRight,
    /// `0x40`, `StrafeLeftStart`/`Stop` (`0x513fa0`/`0x513fd0`).
    StrafeLeft,
    /// `0x80`, `StrafeRightStart`/`Stop` (`0x514000`/`0x514030`).
    StrafeRight,
    /// `0x1`, `TurnOrActionStart`/`Stop` (`0x514120`/`0x514160`), the right mouse button's bit.
    TurnOrAction,
    /// `0x2`, `CameraOrSelectOrMoveStart`/`Stop` (`0x514190`/`0x5141d0`), the left button's.
    CameraOrSelectOrMove,
    /// `0x400`, `PitchUpStart`/`Stop` (`0x514060`/`0x514090`).
    PitchUp,
    /// `0x800`, `PitchDownStart`/`Stop` (`0x5140c0`/`0x5140f0`).
    PitchDown,
}

/// One of the camera's six keyboard axes (`0x5112d0`), in the order of the index `0x50fd40` and
/// `0x50fd90` take: the `MoveView*` pairs of the camera table `0x84f7a0` (`0x50b4a0`-`0x50b590`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewAxis {
    /// 0, `MoveViewInStart`/`Stop`: the distance falls.
    In,
    /// 1, `MoveViewOutStart`/`Stop`: the distance rises.
    Out,
    /// 2, `MoveViewRightStart`/`Stop`: the yaw rises.
    Right,
    /// 3, `MoveViewLeftStart`/`Stop`: the yaw falls.
    Left,
    /// 4, `MoveViewUpStart`/`Stop`: the pitch rises, the camera climbing to look down.
    Up,
    /// 5, `MoveViewDownStart`/`Stop`: the pitch falls.
    Down,
}

/// A call for the camera's look session, in call order: the calls that pass no gate, and the
/// fourth argument a mouse bit's release hands `0x515090`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookCall {
    /// `MouselookStart` (`true`) or `MouselookStop`.
    Mouselook(bool),
    /// `ToggleMouseMove()` (`0x50b5a0` → `0x50f910`): leave mouselook when the camera is in it,
    /// else enter it with no button held.
    ToggleMouseMove,
    /// A gated `TurnOrActionStop` (`false`) or `CameraOrSelectOrMoveStop(sticky)`: the argument
    /// `0x514b70` passes to `0x50f940` when the release leaves no mouse bit held (`0x514c48`).
    MouseRelease { sticky: bool },
}

/// A posture verb the host applies inside its call ([`ScriptCall::Posture`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostureCall {
    /// `SitOrStand()` (`0x48b920`): `SetStandState(1)` on the active player when its stand state
    /// byte is 0, else `SetStandState(0)` (`0x5ed430`).
    SitOrStand,
}

/// A one-shot command, fired as its key fires it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FiredInput {
    /// `Jump()` (`0x513bd0`).
    Jump,
    /// `ToggleRun()` (`0x513d50` → `0x60e080`), the walk toggle.
    ToggleRun,
    /// `ToggleAutoRun()` (`0x513de0`), which toggles `0x1000`.
    ToggleAutoRun,
    /// `ToggleSheath()` (`0x48a070` → `0x5eb480`).
    ToggleSheath,
    /// `CameraZoomIn([yards])` (`0x50b400` → `0x50fc60`), 1.0 when absent.
    CameraZoomIn(f32),
    /// `CameraZoomOut([yards])` (`0x50b450` → `0x50fcd0`), 1.0 when absent.
    CameraZoomOut(f32),
}

/// One call for the app's binding state, in call order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BindingInput {
    Start(HeldInput),
    Stop(HeldInput),
    Fire(FiredInput),
    /// `MoveView*Start`: `0x50fd40(axis, time, 0)`.
    ViewStart(ViewAxis),
    /// `MoveView*Stop`: `0x50fd90(axis, time)`.
    ViewStop(ViewAxis),
}

/// The VM's half of the binding functions.
#[derive(Clone, Default)]
pub(crate) struct InputState {
    /// The calls made since the app last drained them.
    queue: Vec<BindingInput>,
    /// `[InputControl+4] & 1`, the TurnOrAction channel `IsMouselooking` tests (`0x514278`), as
    /// the app pushed it or a `MouselookStart`/`Stop` since moved it.
    turn_or_action: bool,
    /// The look-session calls since the app last drained them.
    look: Vec<LookCall>,
}

impl UiScript {
    /// Take the binding calls queued since the last call, in call order.
    pub fn take_binding_input(&mut self) -> Vec<BindingInput> {
        std::mem::take(&mut self.model_mut().input.queue)
    }

    /// Run a binding body as `CBindings::RunCommand` (`0x4b7b50`) does from inside
    /// `ExecuteBinding`: the movement functions' hardware-event gate passes while it runs.
    pub fn run_binding(&self, chunk: &str) -> mlua::Result<()> {
        let _event = protected::raise(self.lua(), Event::Binding);
        self.run(chunk)
    }

    /// `CBindings::ExecuteBinding` (`0x4b7990`) once the chord has resolved to `command`: its
    /// `RunCommand` (`0x4b7b50`) with the hardware-event gate raised (`0x4b7b02`), a press when
    /// `down`, else the release, which runs only a `runOnUp` body. `Ok(false)` when nothing ran:
    /// no such command, or a release of one that is not `runOnUp`.
    pub fn execute_binding(&self, command: &str, down: bool) -> mlua::Result<bool> {
        let _event = protected::raise(self.lua(), Event::Binding);
        super::host::unheld(&self.lua, || {
            super::keybind::run_command(&self.lua, command, down)
        })
    }

    /// Push whether the TurnOrAction channel is held: the world holds the right mouse button, or
    /// a `MouselookStart` holds it with none.
    pub fn set_turn_or_action_held(&mut self, held: bool) {
        self.model_mut().input.turn_or_action = held;
    }

    /// Take the look-session calls made since the last call, in call order, for the app's look
    /// session.
    pub fn take_look_calls(&mut self) -> Vec<LookCall> {
        std::mem::take(&mut self.model_mut().input.look)
    }
}

/// The gate `0x494a50(0)`, whether a movement call may go on.
fn gate(lua: &Lua) -> bool {
    protected::gate(lua, Tag::MOVEMENT)
}

fn push(lua: &Lua, input: BindingInput) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    model.input.queue.push(input);
}

fn push_look(lua: &Lua, call: LookCall) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    model.input.look.push(call);
}

fn push_call(lua: &Lua, call: ScriptCall) -> mlua::Result<()> {
    super::calls::submit(lua, call)
}

/// A zoom's optional amount: `0x6f34d0` (is-number, a numeric string passes) then `0x6f3620`,
/// else the stored `1.0f` (`0x50b42d`). A NaN or infinite amount is dropped: what `0x50fc60`'s
/// tick conversion makes of it is not established.
fn zoom_amount(lua: &Lua, v: Option<Value>) -> Option<f32> {
    let n = match v {
        Some(v) => lua.coerce_number(v).ok().flatten().unwrap_or(1.0),
        None => 1.0,
    } as f32;
    n.is_finite().then_some(n)
}

/// Register the 49 functions: the three tables' 48 and `TargetNearestFriend`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The movement table `0x8500b8` ──────────────────────────────────────────────────────
    // Each Start/Stop is `0x494a50(0)` ([`gate`]), then `0x515090(bit, set, time, 0)`; nothing is
    // returned, and no argument is read but `CameraOrSelectOrMoveStop`'s sticky flag (`0x5141f6`),
    // its fourth argument instead of 0.
    let held: [(&str, &str, HeldInput); 10] = [
        (
            "MoveForwardStart",
            "MoveForwardStop",
            HeldInput::MoveForward,
        ),
        (
            "MoveBackwardStart",
            "MoveBackwardStop",
            HeldInput::MoveBackward,
        ),
        ("TurnLeftStart", "TurnLeftStop", HeldInput::TurnLeft),
        ("TurnRightStart", "TurnRightStop", HeldInput::TurnRight),
        ("StrafeLeftStart", "StrafeLeftStop", HeldInput::StrafeLeft),
        (
            "StrafeRightStart",
            "StrafeRightStop",
            HeldInput::StrafeRight,
        ),
        // The mouse pair also arms the world click (`0x514810`); that, and each bit alone, the
        // look session reads off the physical buttons. The two held together are the both-button
        // run, which MOVEANDSTEER's body reaches through them.
        (
            "TurnOrActionStart",
            "TurnOrActionStop",
            HeldInput::TurnOrAction,
        ),
        (
            "CameraOrSelectOrMoveStart",
            "CameraOrSelectOrMoveStop",
            HeldInput::CameraOrSelectOrMove,
        ),
        // The pitch pair (`0x400`/`0x800`, `0x51406a`-`0x5140fa`), which the input tick's pitch
        // emitter `0x515010` reads while the mover swims.
        ("PitchUpStart", "PitchUpStop", HeldInput::PitchUp),
        ("PitchDownStart", "PitchDownStop", HeldInput::PitchDown),
    ];
    for (start, stop, input) in held {
        g.set(
            start,
            lua.create_function(move |lua, _: MultiValue| {
                if gate(lua) {
                    push(lua, BindingInput::Start(input));
                }
                Ok(())
            })?,
        )?;
        g.set(
            stop,
            lua.create_function(move |lua, args: MultiValue| {
                if gate(lua) {
                    push(lua, BindingInput::Stop(input));
                    // A mouse bit's release carries its fourth argument to the camera: the sticky
                    // flag through `0x6f1c10` with default 0 (`0x5141f6`), else 0 (`0x51417b`).
                    match input {
                        HeldInput::CameraOrSelectOrMove => {
                            let sticky = flag_or_default(args.front(), 0) != 0;
                            push_look(lua, LookCall::MouseRelease { sticky });
                        }
                        HeldInput::TurnOrAction => {
                            push_look(lua, LookCall::MouseRelease { sticky: false });
                        }
                        _ => {}
                    }
                }
                Ok(())
            })?,
        )?;
    }
    // `Jump` (`0x513bd0`), `ToggleRun` (`0x513d50`) and `ToggleAutoRun` (`0x513de0`) open on the
    // same gate; their further refusals (a dead or rooted player) are the engine's, as for the key.
    let fired: [(&str, FiredInput); 3] = [
        ("Jump", FiredInput::Jump),
        ("ToggleRun", FiredInput::ToggleRun),
        ("ToggleAutoRun", FiredInput::ToggleAutoRun),
    ];
    for (name, input) in fired {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                if gate(lua) {
                    push(lua, BindingInput::Fire(input));
                }
                Ok(())
            })?,
        )?;
    }
    // `IsMouselooking()` (`0x514270`): `[InputControl+4] & 1`, the TurnOrAction channel, not the
    // camera's freelook bit; 1 or nil (`0x514280`/`0x514293`).
    g.set(
        "IsMouselooking",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.input.turn_or_action))
        })?,
    )?;
    // `MouselookStart`/`MouselookStop` (`0x514210`/`0x514240`) pass no gate: each disarms the
    // pending world click (`0x514810(0)`) and sets or clears the TurnOrAction channel
    // (`0x515090(1, set, now, 0)`), the bit the held right button drives. The bit moves at the
    // call, so `IsMouselooking` answers it at once; the app's look session applies the call.
    for (name, on) in [("MouselookStart", true), ("MouselookStop", false)] {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.input.turn_or_action = on;
                model.input.look.push(LookCall::Mouselook(on));
                Ok(())
            })?,
        )?;
    }

    // ── The camera table `0x84f7a0` ────────────────────────────────────────────────────────
    g.set(
        "CameraZoomIn",
        lua.create_function(|lua, v: Option<Value>| {
            if let Some(yards) = zoom_amount(lua, v) {
                push(lua, BindingInput::Fire(FiredInput::CameraZoomIn(yards)));
            }
            Ok(())
        })?,
    )?;
    g.set(
        "CameraZoomOut",
        lua.create_function(|lua, v: Option<Value>| {
            if let Some(yards) = zoom_amount(lua, v) {
                push(lua, BindingInput::Fire(FiredInput::CameraZoomOut(yards)));
            }
            Ok(())
        })?,
    )?;
    // The `MoveView*` pairs (`0x50b4a0`-`0x50b590`) start and stop an axis on the world frame's
    // camera at the last input time, `0x50fd40(axis, time, 0)` and `0x50fd90(axis, time)`; no
    // gate, no argument read, nothing returned.
    let axes: [(&str, &str, ViewAxis); 6] = [
        ("MoveViewInStart", "MoveViewInStop", ViewAxis::In),
        ("MoveViewOutStart", "MoveViewOutStop", ViewAxis::Out),
        ("MoveViewRightStart", "MoveViewRightStop", ViewAxis::Right),
        ("MoveViewLeftStart", "MoveViewLeftStop", ViewAxis::Left),
        ("MoveViewUpStart", "MoveViewUpStop", ViewAxis::Up),
        ("MoveViewDownStart", "MoveViewDownStop", ViewAxis::Down),
    ];
    for (start, stop, axis) in axes {
        g.set(
            start,
            lua.create_function(move |lua, _: MultiValue| {
                push(lua, BindingInput::ViewStart(axis));
                Ok(())
            })?,
        )?;
        g.set(
            stop,
            lua.create_function(move |lua, _: MultiValue| {
                push(lua, BindingInput::ViewStop(axis));
                Ok(())
            })?,
        )?;
    }
    // `ToggleMouseMove` (`0x50b5a0` → `0x50f910`): no gate, no argument, nothing returned.
    g.set(
        "ToggleMouseMove",
        lua.create_function(|lua, _: MultiValue| {
            push_look(lua, LookCall::ToggleMouseMove);
            Ok(())
        })?,
    )?;

    // ── The main table `0x83de68` ──────────────────────────────────────────────────────────
    // The four `TargetNearest*` shims are byte-identical but for the mode: argument 1 through
    // `0x6f1c10` with default 0 is the reverse flag, then `0x493f60(reverse, mode)`.
    let nearest: [(&str, NearestMode); 4] = [
        ("TargetNearestEnemy", NearestMode::Enemy),
        ("TargetNearestFriend", NearestMode::Friend),
        ("TargetNearestPartyMember", NearestMode::PartyMember),
        ("TargetNearestRaidMember", NearestMode::RaidMember),
    ];
    for (name, mode) in nearest {
        g.set(
            name,
            lua.create_function(move |lua, reverse: Option<Value>| {
                // `0x493f9b`: tag 4, ahead of the TAB cycle, refused with nothing signalled.
                if !protected::gate(lua, Tag::TARGET) {
                    return Ok(());
                }
                let reverse = bool_or_default(reverse.as_ref(), false);
                let may_cast = protected::select_may_cast(lua);
                push_call(
                    lua,
                    ScriptCall::TargetNearest {
                        mode,
                        reverse,
                        may_cast,
                    },
                )?;
                Ok(())
            })?,
        )?;
    }
    // `TargetLastTarget()` (`0x489b00`) and `AttackTarget()` (`0x489b50`) read no argument.
    g.set(
        "TargetLastTarget",
        lua.create_function(|lua, _: MultiValue| {
            let may_cast = protected::select_may_cast(lua);
            push_call(lua, ScriptCall::TargetLastTarget { may_cast })?;
            Ok(())
        })?,
    )?;
    g.set(
        "AttackTarget",
        lua.create_function(|lua, _: MultiValue| {
            // The validator's pick of the nearest enemy asks the gate's tag 4 (`0x493f9b`); the rest
            // of the attack has no gate.
            let may_pick = protected::gate(lua, Tag::TARGET);
            let may_cast = protected::select_may_cast(lua);
            push_call(lua, ScriptCall::AttackTarget { may_pick, may_cast })?;
            Ok(())
        })?,
    )?;
    // `ToggleSheath()` and `SitOrStand()` resolve the active player and act on it, ungated.
    g.set(
        "ToggleSheath",
        lua.create_function(move |lua, _: MultiValue| {
            push(lua, BindingInput::Fire(FiredInput::ToggleSheath));
            Ok(())
        })?,
    )?;
    // `SitOrStand()` (`0x48b920`): the host runs `SetStandState` inside the call, a sit when the
    // stand state byte is 0, else a stand (`0x48b951`, `0x48b95b` → `0x5ed430`).
    g.set(
        "SitOrStand",
        lua.create_function(|lua, _: MultiValue| {
            push_call(lua, ScriptCall::Posture(PostureCall::SitOrStand))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three reference tables' 48 names (`0x8500b8`, `0x84f7a0`, and the main table's seven
    /// at `0x83de68`), each a callable global that answers without raising.
    const NAMES: [&str; 48] = [
        "Jump",
        "ToggleRun",
        "ToggleAutoRun",
        "MoveForwardStart",
        "MoveForwardStop",
        "MoveBackwardStart",
        "MoveBackwardStop",
        "TurnLeftStart",
        "TurnLeftStop",
        "TurnRightStart",
        "TurnRightStop",
        "StrafeLeftStart",
        "StrafeLeftStop",
        "StrafeRightStart",
        "StrafeRightStop",
        "PitchUpStart",
        "PitchUpStop",
        "PitchDownStart",
        "PitchDownStop",
        "TurnOrActionStart",
        "TurnOrActionStop",
        "CameraOrSelectOrMoveStart",
        "CameraOrSelectOrMoveStop",
        "MouselookStart",
        "MouselookStop",
        "IsMouselooking",
        "CameraZoomIn",
        "CameraZoomOut",
        "MoveViewInStart",
        "MoveViewInStop",
        "MoveViewOutStart",
        "MoveViewOutStop",
        "MoveViewRightStart",
        "MoveViewRightStop",
        "MoveViewLeftStart",
        "MoveViewLeftStop",
        "MoveViewUpStart",
        "MoveViewUpStop",
        "MoveViewDownStart",
        "MoveViewDownStop",
        "ToggleMouseMove",
        "TargetNearestEnemy",
        "TargetNearestPartyMember",
        "TargetNearestRaidMember",
        "TargetLastTarget",
        "AttackTarget",
        "ToggleSheath",
        "SitOrStand",
    ];

    #[test]
    fn every_binding_function_is_a_global_that_answers() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        for name in NAMES {
            let kind: String = s.eval(&format!("return type({name})")).unwrap();
            assert_eq!(kind, "function", "{name}");
            s.run(&format!("{name}()"))
                .unwrap_or_else(|e| panic!("{name}() raised: {e}"));
            s.run_binding(&format!("{name}(1)"))
                .unwrap_or_else(|e| panic!("{name}(1) in a binding raised: {e}"));
        }
    }

    /// The gate `0x494a50(0)`: a movement call queues only from inside a binding body, while the
    /// camera and posture verbs, which have no gate, queue from anywhere.
    #[test]
    fn the_movement_calls_act_only_inside_a_binding_body() {
        let mut s = UiScript::new().unwrap();
        s.run("MoveForwardStart() StrafeLeftStop() Jump() ToggleRun()")
            .unwrap();
        assert!(s.take_binding_input().is_empty());
        s.run_binding("MoveForwardStart() StrafeLeftStop() Jump() ToggleRun()")
            .unwrap();
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Start(HeldInput::MoveForward),
                BindingInput::Stop(HeldInput::StrafeLeft),
                BindingInput::Fire(FiredInput::Jump),
                BindingInput::Fire(FiredInput::ToggleRun),
            ]
        );
        // The gate closes again after the body, and after a body that raised.
        assert!(s.run_binding("error('boom')").is_err());
        s.run("TurnLeftStart()").unwrap();
        assert!(s.take_binding_input().is_empty());
        s.keep_calls();
        s.run("SitOrStand() ToggleSheath() CameraZoomIn()").unwrap();
        assert_eq!(
            s.take_calls_like(&ScriptCall::Posture(PostureCall::SitOrStand)),
            1
        );
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Fire(FiredInput::ToggleSheath),
                BindingInput::Fire(FiredInput::CameraZoomIn(1.0)),
            ]
        );
    }

    /// The zoom's amount: a number or numeric string (`0x6f34d0`), else 1.0 (`0x50b42d`); a
    /// non-finite amount is dropped.
    #[test]
    fn a_zoom_takes_its_yards_or_one() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"CameraZoomIn(2.5) CameraZoomOut("4") CameraZoomIn("far") CameraZoomOut(0/0)"#)
            .unwrap();
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Fire(FiredInput::CameraZoomIn(2.5)),
                BindingInput::Fire(FiredInput::CameraZoomOut(4.0)),
                BindingInput::Fire(FiredInput::CameraZoomIn(1.0)),
            ]
        );
    }

    /// `IsMouselooking` answers the pushed TurnOrAction channel as 1 or nil.
    #[test]
    fn is_mouselooking_answers_one_or_nil() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("IsMouselooking()").unwrap(), 1);
        assert!(s
            .eval::<Option<i64>>("return IsMouselooking()")
            .unwrap()
            .is_none());
        s.set_turn_or_action_held(true);
        assert_eq!(
            s.eval::<Option<i64>>("return IsMouselooking()").unwrap(),
            Some(1)
        );
    }

    /// `MouselookStart`/`Stop` pass no gate (`0x514210`/`0x514240`), queue for the look session in
    /// call order, and move the channel at the call, so `IsMouselooking` answers them at once.
    #[test]
    fn mouselook_start_and_stop_move_the_channel_from_any_caller() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("MouselookStart()").unwrap(), 0);
        s.run("MouselookStop()").unwrap();
        s.take_look_calls();
        let looking = |s: &UiScript| s.eval::<Option<i64>>("return IsMouselooking()").unwrap();
        s.run("MouselookStart()").unwrap();
        assert_eq!(
            looking(&s),
            Some(1),
            "an addon's own call, outside any binding"
        );
        s.run("MouselookStop() MouselookStart() MouselookStop()")
            .unwrap();
        assert_eq!(looking(&s), None);
        assert_eq!(
            s.take_look_calls(),
            [true, false, true, false].map(LookCall::Mouselook)
        );
        assert!(s.take_look_calls().is_empty(), "drained");
        assert!(
            s.take_binding_input().is_empty(),
            "not a movement-binding call"
        );
        // The app's push is the channel's truth from then on: a right release ended it.
        s.run("MouselookStart()").unwrap();
        s.set_turn_or_action_held(false);
        assert_eq!(looking(&s), None);
    }

    /// The `MoveView*` pairs (`0x50b4a0`-`0x50b590`) pass no gate and queue their axis, by the
    /// index the camera table `0x84f7a0` gives each name: Right is 2 (`0x50b520`), Left 3.
    #[test]
    fn the_move_view_pairs_queue_their_axis_from_any_caller() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "MoveViewInStart() MoveViewOutStop() MoveViewRightStart() MoveViewLeftStop() \
             MoveViewUpStart() MoveViewDownStop()",
        )
        .unwrap();
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::ViewStart(ViewAxis::In),
                BindingInput::ViewStop(ViewAxis::Out),
                BindingInput::ViewStart(ViewAxis::Right),
                BindingInput::ViewStop(ViewAxis::Left),
                BindingInput::ViewStart(ViewAxis::Up),
                BindingInput::ViewStop(ViewAxis::Down),
            ]
        );
        for name in ["MoveViewInStart", "MoveViewDownStop"] {
            assert_eq!(s.arity(&format!("{name}()")).unwrap(), 0, "{name}");
        }
    }

    /// `ToggleMouseMove` (`0x50b5a0`) passes no gate and queues for the look session; it moves no
    /// mouse bit, so `IsMouselooking` does not answer it.
    #[test]
    fn toggle_mouse_move_queues_for_the_look_session() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("ToggleMouseMove()").unwrap(), 0);
        s.take_look_calls();
        s.run("ToggleMouseMove() ToggleMouseMove()").unwrap();
        assert_eq!(
            s.take_look_calls(),
            vec![LookCall::ToggleMouseMove, LookCall::ToggleMouseMove]
        );
        assert!(s
            .eval::<Option<i64>>("return IsMouselooking()")
            .unwrap()
            .is_none());
    }

    /// `CameraOrSelectOrMoveStop`'s flag (`0x6f1c10` at `0x5141f6`, default 0) rides its release to
    /// the camera; `TurnOrActionStop` hands 0 (`0x51417b`). Both behind the movement gate.
    #[test]
    fn a_mouse_release_carries_the_sticky_flag_to_the_camera() {
        let mut s = UiScript::new().unwrap();
        s.run("CameraOrSelectOrMoveStop(1)").unwrap();
        assert!(
            s.take_look_calls().is_empty(),
            "the gate refuses it outside a binding"
        );
        s.run_binding(
            "CameraOrSelectOrMoveStop(1) CameraOrSelectOrMoveStop() \
             CameraOrSelectOrMoveStop(\"0\") TurnOrActionStop(1) CameraOrSelectOrMoveStart(1)",
        )
        .unwrap();
        assert_eq!(
            s.take_look_calls(),
            vec![
                LookCall::MouseRelease { sticky: true },
                LookCall::MouseRelease { sticky: false },
                LookCall::MouseRelease { sticky: false },
                LookCall::MouseRelease { sticky: false },
            ]
        );
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Stop(HeldInput::CameraOrSelectOrMove),
                BindingInput::Stop(HeldInput::CameraOrSelectOrMove),
                BindingInput::Stop(HeldInput::CameraOrSelectOrMove),
                BindingInput::Stop(HeldInput::TurnOrAction),
                BindingInput::Start(HeldInput::CameraOrSelectOrMove),
            ]
        );
    }

    /// The pitch pair presses and releases bits `0x400` and `0x800` (`0x514060`, `0x5140c0`)
    /// behind the movement gate.
    #[test]
    fn the_pitch_pair_moves_its_bits_inside_a_binding_body() {
        let mut s = UiScript::new().unwrap();
        s.run("PitchUpStart()").unwrap();
        assert!(s.take_binding_input().is_empty());
        s.run_binding("PitchUpStart() PitchDownStart() PitchUpStop() PitchDownStop()")
            .unwrap();
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Start(HeldInput::PitchUp),
                BindingInput::Start(HeldInput::PitchDown),
                BindingInput::Stop(HeldInput::PitchUp),
                BindingInput::Stop(HeldInput::PitchDown),
            ]
        );
    }

    /// `TargetLastTarget` and `AttackTarget` are script calls, made in call order with the rest.
    #[test]
    fn the_targeting_calls_join_the_call_queue_in_order() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run(r#"TargetNearestEnemy(1) TargetLastTarget() AttackTarget() ClearTarget()"#)
            .unwrap();
        assert_eq!(
            s.take_script_calls(),
            vec![
                ScriptCall::TargetNearest {
                    mode: NearestMode::Enemy,
                    reverse: true,
                    may_cast: true
                },
                ScriptCall::TargetLastTarget { may_cast: true },
                ScriptCall::AttackTarget {
                    may_pick: true,
                    may_cast: true
                },
                ScriptCall::ClearTarget,
            ]
        );
    }

    /// The TAB cyclers, `TargetLastTarget` and `AttackTarget` apply inside their call with a host
    /// lent.
    #[test]
    fn the_targeting_keys_apply_in_the_call_with_a_host_lent() {
        let mut s = UiScript::new().unwrap();
        let calls = crate::script::host::test_support::applied_in_call(
            &mut s,
            "TargetNearestEnemy() TargetNearestFriend() TargetNearestPartyMember() \
             TargetNearestRaidMember() TargetLastTarget() AttackTarget()",
        );
        let modes: Vec<_> = calls
            .iter()
            .filter_map(|c| match c {
                ScriptCall::TargetNearest { mode, .. } => Some(*mode),
                _ => None,
            })
            .collect();
        assert_eq!(
            modes,
            [
                NearestMode::Enemy,
                NearestMode::Friend,
                NearestMode::PartyMember,
                NearestMode::RaidMember
            ]
        );
        assert!(matches!(
            calls[4..],
            [
                ScriptCall::TargetLastTarget { may_cast: true },
                ScriptCall::AttackTarget {
                    may_pick: true,
                    may_cast: true
                }
            ]
        ));
    }
}
