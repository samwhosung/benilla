//! The four tutorial verbs, registered at `0x846b04`. The banks are the app's: it pushes the
//! acknowledged bank for `TutorialsEnabled`, and each writer hands its [`TutorialCall`] to the call
//! host, which writes the banks and sends before the verb returns. `FlagTutorial(n)` acknowledges
//! (`0x4b59dc` → `0x4b54c0`); `ClearTutorials()` sets every bit of both banks and sends
//! `CMSG_TUTORIAL_CLEAR` (`0x4b5a00` → `0x4b55c0`); `ResetTutorials()` clears them and sends
//! `CMSG_TUTORIAL_RESET` (`0x4b5a10` → `0x4b5660`). None of the three writers returns a value.

use mlua::{Lua, Value};

use super::binding_abi::{flag, number_arg};
use super::Model;

/// `FlagTutorial`'s bound, `0 ≤ n − 1 < 0x32`: the fifty tutorials `GlobalStrings.lua` names.
const TUTORIAL_IDS: i32 = 0x32;

/// A tutorial writer, which the call host applies inside the call
/// ([`super::ScriptCall::Tutorial`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TutorialCall {
    /// `FlagTutorial(n)`: `AcknowledgeTutorial 0x4b54c0` on the 0-based id `n − 1`.
    Flag(u32),
    /// `ClearTutorials()`: `ClearAllTutorials 0x4b55c0`.
    Clear,
    /// `ResetTutorials()`: `ResetAllTutorials 0x4b5660`.
    Reset,
}

impl super::UiScript {
    /// The acknowledged bank's bytes, `None` before `SMSG_TUTORIAL_FLAGS`.
    pub fn set_tutorial_bank(&mut self, bank: Option<Vec<u8>>) {
        self.model_mut().tutorial_bank = bank;
    }

    /// The tutorial calls a harness VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_tutorial_calls(&mut self) -> Vec<TutorialCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Tutorial(call) => Some(*call),
            _ => None,
        })
    }

    /// A `TriggerTutorial(id, 0)` (`0x4b5390`) the engine makes beside a script, as a 0-based id,
    /// drained with the VM's own ([`Self::take_tutorial_triggers`]).
    pub fn queue_tutorial_trigger(&mut self, id: u32) {
        self.model_mut().tutorial_triggers.push(id);
    }

    /// `TriggerTutorial(id, 0)` asks the engine made since the last drain, as 0-based ids, for the
    /// app's fire-once bank.
    pub fn take_tutorial_triggers(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().tutorial_triggers)
    }
}

/// Hand `call` to the host, which applies it before the verb returns.
fn submit(lua: &Lua, call: TutorialCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Tutorial(call))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `TutorialsEnabled()` (`0x4b5960`): 1 if any of the bank's `[0xb711ec] * 4` bytes is not
    // 0xFF, the whole bank and not just the fifty ids; else nil, as with no bank.
    g.set(
        "TutorialsEnabled",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let enabled = model
                .tutorial_bank
                .as_ref()
                .is_some_and(|b| b.iter().any(|&byte| byte != 0xFF));
            Ok(flag(enabled))
        })?,
    )?;

    // `FlagTutorial(n)` (`0x4b59b0`): a non-number raises the reference's usage text, which asks
    // for a string; truncated, an `n − 1` outside `0..0x32` is a silent no-op.
    g.set(
        "FlagTutorial",
        lua.create_function(|lua, n: Value| {
            let n = number_arg(lua, n, "Usage: FlagTutorial(\"tutorial\")")?;
            let id = n.wrapping_sub(1);
            if !(0..TUTORIAL_IDS).contains(&id) {
                return Ok(());
            }
            submit(lua, TutorialCall::Flag(id as u32))
        })?,
    )?;

    g.set(
        "ClearTutorials",
        lua.create_function(|lua, ()| submit(lua, TutorialCall::Clear))?,
    )?;

    g.set(
        "ResetTutorials",
        lua.create_function(|lua, ()| submit(lua, TutorialCall::Reset))?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn tutorials_enabled_scans_the_whole_acknowledged_bank() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("TutorialsEnabled()").unwrap(), 1);
        assert!(
            s.eval::<bool>("return TutorialsEnabled() == nil").unwrap(),
            "no bank"
        );
        s.set_tutorial_bank(Some(vec![0xFF; 32]));
        assert!(
            s.eval::<bool>("return TutorialsEnabled() == nil").unwrap(),
            "every bit acknowledged: disabled"
        );
        let mut bank = vec![0xFF; 32];
        bank[31] = 0x7F; // a bit past the fifty real ids
        s.set_tutorial_bank(Some(bank));
        assert_eq!(
            s.eval::<i64>("return TutorialsEnabled()").unwrap(),
            1,
            "the scan covers the whole bank, not the 50 ids"
        );
        s.set_tutorial_bank(Some(vec![0; 32]));
        assert_eq!(s.eval::<i64>("return TutorialsEnabled()").unwrap(), 1);
    }

    #[test]
    fn flag_tutorial_takes_a_one_based_number_and_bails_silently_off_the_fifty() {
        use super::TutorialCall::{Clear, Flag, Reset};
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("FlagTutorial(1) FlagTutorial(50) FlagTutorial('42') FlagTutorial(7.9) FlagTutorial(0) FlagTutorial(51) FlagTutorial(-3)")
            .unwrap();
        assert_eq!(
            s.take_tutorial_calls(),
            vec![Flag(0), Flag(49), Flag(41), Flag(6)],
            "0-based, truncated toward zero, 0/51/−3 dropped"
        );
        assert!(s.take_tutorial_calls().is_empty(), "drained");
        for call in ["FlagTutorial()", "FlagTutorial(nil)", "FlagTutorial({})"] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(
                err.contains("Usage: FlagTutorial(\"tutorial\")"),
                "{call}: {err}"
            );
        }
        s.run("ClearTutorials() ClearTutorials() ResetTutorials()")
            .unwrap();
        assert_eq!(s.take_tutorial_calls(), vec![Clear, Clear, Reset]);
    }
}
