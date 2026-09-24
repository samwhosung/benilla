//! The tutorial system's four Lua verbs (decision 1976): the registrar table at `.data 0x846b04` —
//! `TutorialsEnabled`, `FlagTutorial`, `ClearTutorials`, `ResetTutorials`.
//!
//! The banks are the app's (they are wire state, and the fire-once trigger law lives beside the
//! sites that trigger): the app pushes the **acknowledged** bank's bytes here when they change,
//! which is all `TutorialsEnabled` reads, and drains the three writes.
//!
//! | verb | shape |
//! |---|---|
//! | `TutorialsEnabled()` | one value on both legs: the number `1` if ANY byte of the acknowledged bank is not `0xFF`, else `nil` — over the whole bank, not the 50 real ids; `nil` with no bank |
//! | `FlagTutorial(n)` | `n` must pass `lua_isnumber` else `Usage: FlagTutorial("tutorial")` (the quoted usage is the reference's own, and misleading — a number is required); truncated, `n − 1` must lie in `0..50` else a SILENT no-op; zero values |
//! | `ClearTutorials()` | zero values; every bit set in both banks and `CMSG_TUTORIAL_CLEAR` |
//! | `ResetTutorials()` | zero values; every bit cleared in both banks and `CMSG_TUTORIAL_RESET` |

use mlua::{Lua, Value};

use super::binding_abi::{flag, number_arg};
use super::Model;

/// `FlagTutorial`'s clamp: `0 ≤ n − 1 < 0x32` — the fifty ids `GlobalStrings.lua` names.
const TUTORIAL_IDS: i32 = 0x32;

impl super::UiScript {
    /// The acknowledged bank's bytes (`None` before `SMSG_TUTORIAL_FLAGS` lands) — what
    /// `TutorialsEnabled()` scans.
    pub fn set_tutorial_bank(&mut self, bank: Option<Vec<u8>>) {
        self.model_mut().tutorial_bank = bank;
    }

    /// `FlagTutorial(n)` calls since the last drain: the **0-based** ids (`n − 1`), in range.
    pub fn take_tutorial_flag_requests(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().tutorial_flag_requests)
    }

    /// `ClearTutorials()` calls since the last drain.
    pub fn take_tutorial_clears(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().tutorial_clears)
    }

    /// `ResetTutorials()` calls since the last drain.
    pub fn take_tutorial_resets(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().tutorial_resets)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `TutorialsEnabled()` (`0x4b5960`): scans `[0xb711ec] * 4` bytes of bank B for one ≠ 0xFF;
    // `1.0` or nil, one value; an unallocated bank skips the loop and answers nil.
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

    // `FlagTutorial(n)` (`0x4b59b0`): `lua_isnumber` else the Usage raise; `__ftol`; `dec`; `js`
    // and `cmp 0x32 / jge` bail silently; else the acknowledge-and-send setter with the 0-based id.
    g.set(
        "FlagTutorial",
        lua.create_function(|lua, n: Value| {
            let n = number_arg(lua, n, "Usage: FlagTutorial(\"tutorial\")")?;
            let id = n.wrapping_sub(1);
            if !(0..TUTORIAL_IDS).contains(&id) {
                return Ok(());
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.tutorial_flag_requests.push(id as u32);
            Ok(())
        })?,
    )?;

    g.set(
        "ClearTutorials",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.tutorial_clears += 1;
            Ok(())
        })?,
    )?;

    g.set(
        "ResetTutorials",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.tutorial_resets += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// `1` or nil, one value, over the WHOLE bank — and nil with none.
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

    /// `FlagTutorial`: the number gate's raise, `n − 1`, the silent out-of-range legs, truncation.
    #[test]
    fn flag_tutorial_takes_a_one_based_number_and_bails_silently_off_the_fifty() {
        let mut s = UiScript::new().unwrap();
        s.run("FlagTutorial(1) FlagTutorial(50) FlagTutorial('42') FlagTutorial(7.9) FlagTutorial(0) FlagTutorial(51) FlagTutorial(-3)")
            .unwrap();
        assert_eq!(
            s.take_tutorial_flag_requests(),
            vec![0, 49, 41, 6],
            "0-based, truncated toward zero, 0/51/−3 dropped"
        );
        assert!(s.take_tutorial_flag_requests().is_empty(), "drained");
        for call in ["FlagTutorial()", "FlagTutorial(nil)", "FlagTutorial({})"] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(
                err.contains("Usage: FlagTutorial(\"tutorial\")"),
                "{call}: {err}"
            );
        }
        s.run("ClearTutorials() ClearTutorials() ResetTutorials()")
            .unwrap();
        assert_eq!(s.take_tutorial_clears(), 2);
        assert_eq!(s.take_tutorial_resets(), 1);
        assert_eq!(s.take_tutorial_clears(), 0);
    }
}
