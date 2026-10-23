//! Lua globals whose whole body pushes one catalog id at `CGGameUI::DisplayError`'s dispatcher,
//! which shows the line inside the call ([`super::calls::display_error`]): the app resolves the
//! key against GlobalStrings and signals `UI_ERROR_MESSAGE` (event `0xe0`) before the verb
//! returns, the reference's route.
//!
//! `NotWhileDeadError` (`0x48d340`, registered at `0x83e398`) is `push 0x7e; call 0x496720`: no
//! argument, no dead check, no return, and a silent catalog row (`0xb4be70`). FrameXML decides
//! when to call it (`UIParent.lua:663-666`, `ContainerFrame.lua:147` and `:190`).

use mlua::Lua;

/// Catalog id `0x7e`'s GlobalStrings key ("You can't do that when you're dead.").
const NOT_WHILE_DEAD_KEY: &str = "ERR_PLAYER_DEAD";

/// Register the error-display globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "NotWhileDeadError",
        lua.create_function(|lua, ()| super::calls::display_error(lua, NOT_WHILE_DEAD_KEY))?,
    )
}

/// A host for the tests of a verb's refusal: it shows each `DisplayError` as the app's does,
/// signalling `UI_ERROR_MESSAGE` with the key inside the call, and keeps every other call.
#[cfg(test)]
pub(crate) mod shown {
    use crate::script::{CallHost, CallTail, ScriptCall, ScriptValue, UiScript};

    /// The host: the calls it kept, `DisplayError` aside.
    #[derive(Default)]
    pub(crate) struct Shows {
        pub(crate) calls: Vec<ScriptCall>,
    }

    impl CallHost for Shows {
        fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
            match call {
                ScriptCall::DisplayError(key) => {
                    script.fire_event("UI_ERROR_MESSAGE", vec![ScriptValue::Str(key.into())]);
                }
                call => self.calls.push(call),
            }
            None
        }
    }

    /// What the line after `chunk` saw of the lines its verbs showed, run with [`Shows`] lent: the
    /// keys, comma-joined, and the host with the calls it kept.
    pub(crate) fn seen_in_call(s: &mut UiScript, chunk: &str) -> (String, Shows) {
        s.run(
            r#"SHOWN = {}
            if not SHOWN_TAP then
                SHOWN_TAP = CreateFrame("Frame")
                SHOWN_TAP:RegisterEvent("UI_ERROR_MESSAGE")
                SHOWN_TAP:SetScript("OnEvent", function() table.insert(SHOWN, arg1) end)
            end"#,
        )
        .unwrap();
        let mut host = Shows::default();
        s.hosted(&mut host, |s| {
            s.run(&format!(r#"{chunk} SEEN = table.concat(SHOWN, ",")"#))
        })
        .unwrap();
        let seen: String = s.lua().globals().get("SEEN").unwrap();
        (seen, host)
    }
}

#[cfg(test)]
mod tests {
    use super::shown::seen_in_call;
    use crate::script::UiScript;

    /// `NotWhileDeadError` (`0x48d340`) is `DisplayError(0x7e)`, which signals before it returns,
    /// so the chunk's next line reads the line, as `ContainerFrame.lua:147` goes on after it.
    #[test]
    fn not_while_dead_error_shows_inside_its_call() {
        let mut s = UiScript::new().unwrap();
        let (seen, _) = seen_in_call(&mut s, "NotWhileDeadError()");
        assert_eq!(seen, "ERR_PLAYER_DEAD");
        assert!(s.take_ui_errors().is_empty(), "nothing waits for the feed");
    }
}
