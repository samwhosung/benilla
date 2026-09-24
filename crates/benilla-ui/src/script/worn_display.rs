//! `ShowHelm`/`ShowingHelm` and `ShowCloak`/`ShowingCloak`, over the `PLAYER_FLAGS` bits
//! `HIDE_HELM` (`0x400`) and `HIDE_CLOAK` (`0x800`), which the server keeps per character, so no
//! CVar backs them. The wire verbs are bodiless flips (vmangos `HandleShowingHelmOpcode` is a bare
//! `ToggleFlag`), so a set queues a flip only when it differs from a belief that moves at the call
//! and is overwritten when the wire bit moves.

use mlua::{Lua, Value};

use super::Model;

/// A queued flip: one `CMSG_TOGGLE_HELM` or `CMSG_TOGGLE_CLOAK` for the app to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WornDisplay {
    /// The head slot, `PLAYER_FLAGS_HIDE_HELM`.
    Helm,
    /// The back slot, `PLAYER_FLAGS_HIDE_CLOAK`.
    Cloak,
}

impl super::UiScript {
    /// Drain the queued flips, one packet each; two flips of one slot in a frame are two sends.
    pub fn take_worn_display_toggles(&mut self) -> Vec<WornDisplay> {
        std::mem::take(&mut self.model_mut().worn_display_toggles)
    }

    /// Push the wire truth, only on a `PLAYER_FLAGS` edge: pushed per frame, it would undo an
    /// optimistic flip before the server answers, and a second click would flip the wrong way.
    pub fn set_worn_display(&mut self, helm_shown: bool, cloak_shown: bool) {
        let mut model = self.model_mut();
        model.helm_shown = helm_shown;
        model.cloak_shown = cloak_shown;
    }

    /// What the VM currently believes, `(helm_shown, cloak_shown)`.
    pub fn worn_display(&self) -> (bool, bool) {
        let model = self.model_ref();
        (model.helm_shown, model.cloak_shown)
    }
}

/// Numeric, not Lua-truthy: the stock panel passes the string `"0"` for off
/// (`UIOptionsFrame_Save`), which Lua 5.0 holds truthy. Nil, false, 0 and `"0"` hide.
fn shown_arg(v: &Value) -> bool {
    match v {
        Value::Nil => false,
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Number(n) => *n != 0.0,
        // A non-numeric string is never handed to this API; it shows rather than hiding gear.
        Value::String(s) => s
            .to_str()
            .ok()
            .and_then(|t| t.trim().parse::<f64>().ok())
            .is_none_or(|n| n != 0.0),
        _ => true,
    }
}

/// Ask for a state, and queue the flip only if we are not already in it.
fn want(model: &mut Model, which: WornDisplay, show: bool) {
    let held = match which {
        WornDisplay::Helm => &mut model.helm_shown,
        WornDisplay::Cloak => &mut model.cloak_shown,
    };
    if *held == show {
        return;
    }
    *held = show; // optimistic: the wire edge overwrites it when the server answers
    model.worn_display_toggles.push(which);
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    for (name, which) in [
        ("ShowHelm", WornDisplay::Helm),
        ("ShowCloak", WornDisplay::Cloak),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, v: Value| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                want(&mut model, which, shown_arg(&v));
                Ok(())
            })?,
        )?;
    }

    // `1` or nil, which the Options row feeds straight into `SetChecked`.
    for (name, which) in [
        ("ShowingHelm", WornDisplay::Helm),
        ("ShowingCloak", WornDisplay::Cloak),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let shown = match which {
                    WornDisplay::Helm => model.helm_shown,
                    WornDisplay::Cloak => model.cloak_shown,
                };
                Ok(shown.then_some(1u32))
            })?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::WornDisplay;
    use crate::script::UiScript;

    #[test]
    fn the_setter_reads_its_argument_numerically_so_the_string_zero_hides() {
        let mut s = UiScript::new().unwrap();
        assert!(
            s.eval::<bool>("return ShowingHelm() == 1").unwrap(),
            "shown by default"
        );

        s.run(r#"ShowHelm("0")"#).unwrap();
        assert!(s.eval::<bool>("return ShowingHelm() == nil").unwrap());
        assert_eq!(s.take_worn_display_toggles(), vec![WornDisplay::Helm]);

        s.run(r#"ShowHelm("1")"#).unwrap();
        assert!(s.eval::<bool>("return ShowingHelm() == 1").unwrap());
        assert_eq!(s.take_worn_display_toggles(), vec![WornDisplay::Helm]);

        // The other spellings the API is handed elsewhere in 1.12: a bare number and nil.
        s.run("ShowCloak(0)").unwrap();
        assert!(s.eval::<bool>("return ShowingCloak() == nil").unwrap());
        s.run("ShowCloak(1)").unwrap();
        assert!(s.eval::<bool>("return ShowingCloak() == 1").unwrap());
        s.run("ShowCloak(nil)").unwrap();
        assert!(s.eval::<bool>("return ShowingCloak() == nil").unwrap());
        assert_eq!(
            s.take_worn_display_toggles(),
            vec![WornDisplay::Cloak, WornDisplay::Cloak, WornDisplay::Cloak]
        );
    }

    /// The Options window's Defaults button writes every row, so a flip on no difference would
    /// invert a preference already at its default.
    #[test]
    fn asking_for_the_state_we_are_already_in_sends_nothing() {
        let mut s = UiScript::new().unwrap();
        s.run("ShowHelm(1) ShowCloak(1)").unwrap();
        assert!(
            s.take_worn_display_toggles().is_empty(),
            "both already shown — no packet"
        );

        s.run("ShowHelm(0)").unwrap();
        assert_eq!(s.take_worn_display_toggles(), vec![WornDisplay::Helm]);
        s.run("ShowHelm(0)").unwrap();
        assert!(
            s.take_worn_display_toggles().is_empty(),
            "the belief moved at the first call, so the second is a no-op"
        );
    }

    #[test]
    fn the_wire_push_overrides_the_optimistic_belief() {
        let mut s = UiScript::new().unwrap();
        s.run("ShowHelm(0)").unwrap();
        let _ = s.take_worn_display_toggles();
        assert_eq!(s.worn_display(), (false, true));

        // The server answers otherwise; the descriptor wins and the next ask is computed from it.
        s.set_worn_display(true, false);
        assert_eq!(s.worn_display(), (true, false));
        s.run("ShowHelm(0)").unwrap();
        assert_eq!(
            s.take_worn_display_toggles(),
            vec![WornDisplay::Helm],
            "re-asked against the server's value, not the stale belief"
        );
    }
}
