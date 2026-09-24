//! The **equipment-display** API surface — `ShowHelm`/`ShowingHelm`, `ShowCloak`/`ShowingCloak`:
//! the player's two "don't draw this piece of my gear" preferences.
//!
//! Two bits, and they are not the UI's to keep. The preference lives server-side in the character's
//! own `PLAYER_FLAGS` (`HIDE_HELM 0x400` / `HIDE_CLOAK 0x800`), which is a **public** descriptor
//! field — so it dresses our body on every client that can see us, and every other player's on
//! ours. That is why there is no CVar and no saved variable behind this row: the wire flag is the
//! single source of truth, it is per-character rather than per-install, and it is already persisted
//! by whoever we are logged into.
//!
//! **The setter is a set; the wire verb is a flip.** `CMSG_TOGGLE_HELM`/`CMSG_TOGGLE_CLOAK` carry no
//! body and have no target-state form at all (vmangos `HandleShowingHelmOpcode` is a bare
//! `ToggleFlag`), so `ShowHelm(v)` compares `v` against what we currently believe and queues a flip
//! only on a difference. The belief is updated **optimistically at the call** and overwritten only
//! when the *wire* bit actually moves ([`super::UiScript::set_worn_display`], pushed on the
//! descriptor edge and not per frame) — without that, the value would snap back to the stale
//! descriptor for the length of the round trip and a second click inside that window would compute
//! the wrong flip.
//!
//! The 1.12 panel calls the setter with the **strings** `"1"` / `"0"` (`UIOptionsFrame_Save`
//! l.286-297, `value.setFunc(value.value)`), so the argument is read *numerically* — plain Lua
//! truthiness would make `"0"` mean "show" and the reference's own Options panel could never turn
//! a helm off. See [`shown_arg`].

use mlua::{Lua, Value};

use super::Model;

/// Which of the two worn-display preferences a queued flip is for. One entry per
/// `CMSG_TOGGLE_HELM`/`CMSG_TOGGLE_CLOAK` the app should send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WornDisplay {
    /// The head slot — `PLAYER_FLAGS_HIDE_HELM`.
    Helm,
    /// The back slot — `PLAYER_FLAGS_HIDE_CLOAK`.
    Cloak,
}

impl super::UiScript {
    /// Drain the worn-display flips queued since the last call — one `CMSG_TOGGLE_HELM` /
    /// `CMSG_TOGGLE_CLOAK` each. A list rather than a count because the two slots are distinct
    /// packets; within one slot, two flips in a frame are two sends, exactly like the PvP toggle.
    pub fn take_worn_display_toggles(&mut self) -> Vec<WornDisplay> {
        std::mem::take(&mut self.model_mut().worn_display_toggles)
    }

    /// Push the wire truth for both preferences — the app calls this **on a `PLAYER_FLAGS` edge**,
    /// never per frame, so an optimistic flip made by [`install`]'s setters survives until the
    /// server's answer actually arrives (see the module doc).
    pub fn set_worn_display(&mut self, helm_shown: bool, cloak_shown: bool) {
        let mut model = self.model_mut();
        model.helm_shown = helm_shown;
        model.cloak_shown = cloak_shown;
    }

    /// What the VM currently believes — the app's own read, for the drain's compare and for tests.
    pub fn worn_display(&self) -> (bool, bool) {
        let model = self.model_ref();
        (model.helm_shown, model.cloak_shown)
    }
}

/// The setter's argument convention: **numeric**, not Lua-truthy. `nil`/`false`/`0`/`"0"` read as
/// "hide"; `true`, any non-zero number and any non-numeric string read as "show".
///
/// The distinction is load-bearing rather than pedantic. 1.12's own Interface panel hands this
/// binding the *string* `"0"` to mean off (`UIOptionsFrame_Save`), and in Lua 5.0 `"0"` is truthy —
/// so a plain `lua_toboolean` reading would make the reference's own Show Helm checkbox a one-way
/// switch. Numeric-string coercion is what makes it work, and it is the same shape
/// [`super::action::truthy_nonzero`] already carries for `UseAction`'s `checkCursor`.
fn shown_arg(v: &Value) -> bool {
    match v {
        Value::Nil => false,
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Number(n) => *n != 0.0,
        // A numeric string coerces like Lua's own arithmetic does; a non-numeric one is not a
        // value this API is ever handed, and reads as "show" rather than silently hiding gear.
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
    *held = show; // optimistic — the wire edge overwrites it when the server answers
    model.worn_display_toggles.push(which);
}

/// Register the equipment-display globals.
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

    // The getters the Options row reads its checked state from — `1`/`nil`, the 1.12 boolean
    // return the panel feeds straight into `SetChecked`.
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

    /// The argument convention, and the one that would silently break the feature: 1.12's own
    /// Options panel calls the setter with the **string** `"0"` to mean "hide", and in Lua 5.0 that
    /// string is truthy. A plain truthiness reading would make Show Helm a one-way switch — on
    /// forever, off never — with no error anywhere to say so.
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

    /// The wire verb is a blind flip, so **asking for the state we are already in must send
    /// nothing** — otherwise the Options window's Defaults button, which writes every row on the
    /// page unconditionally, would invert whichever preference was already at its default.
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

    /// The descriptor is the truth: a wire push overrides whatever the VM believed. This is the
    /// edge the app feeds — it is why the app pushes on a `PLAYER_FLAGS` CHANGE and not per frame.
    #[test]
    fn the_wire_push_overrides_the_optimistic_belief() {
        let mut s = UiScript::new().unwrap();
        s.run("ShowHelm(0)").unwrap();
        let _ = s.take_worn_display_toggles();
        assert_eq!(s.worn_display(), (false, true));

        // The server answers something else entirely (another client toggled it, or our flip
        // crossed a login) — the descriptor wins, and the next ask is computed from it.
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
