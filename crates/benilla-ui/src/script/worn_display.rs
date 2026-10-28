//! `ShowHelm`/`ShowingHelm` and `ShowCloak`/`ShowingCloak`, over the `PLAYER_FLAGS` bits
//! `HIDE_HELM` (`0x400`) and `HIDE_CLOAK` (`0x800`), which the server keeps per character, so no
//! CVar backs them. The getters read the live bit (`0x48dd8b`, `0x48ddeb`); the setters hand the
//! host the ask, which re-dresses the model and sends the bodiless toggle only while the live bit
//! says the opposite (`0x5ee660`, `0x5ee740`, `0x5ee7f0`, `0x5ee8d0`).

use mlua::{Lua, MultiValue};

use super::binding_abi::bool_or_default;
use super::stat_read::read_self;

/// The piece a `ShowHelm` or `ShowCloak` call asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WornDisplay {
    /// The head slot, `PLAYER_FLAGS_HIDE_HELM`.
    Helm,
    /// The back slot, `PLAYER_FLAGS_HIDE_CLOAK`.
    Cloak,
}

/// One `ShowHelm(flag)` or `ShowCloak(flag)`, which the host applies inside the call
/// ([`super::ScriptCall::WornDisplay`]): the show method for a true flag, the hide method for a
/// false one (`0x48de59`, `0x48de63`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WornDisplaySet {
    /// The helm or the cloak.
    pub piece: WornDisplay,
    /// The flag, `GetBoolOrDefault`'s answer (`0x48de4e`).
    pub show: bool,
}

impl super::UiScript {
    /// The asks a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_worn_display_sets(&mut self) -> Vec<WornDisplaySet> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::WornDisplay(set) => Some(*set),
            _ => None,
        })
    }

    /// Set the VM's copy of the wire bits, `(helm_shown, cloak_shown)`, for a getter that runs
    /// with no host free to ask ([`super::SelfRead`]).
    pub fn set_worn_display(&mut self, helm_shown: bool, cloak_shown: bool) {
        let mut model = self.model_mut();
        model.helm_shown = helm_shown;
        model.cloak_shown = cloak_shown;
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    for (name, piece) in [
        ("ShowHelm", WornDisplay::Helm),
        ("ShowCloak", WornDisplay::Cloak),
    ] {
        g.set(
            name,
            // `GetBoolOrDefault` (`0x6f1c10`) with default 1 (`0x48de4e`, `0x48de9e`): a bare call
            // shows, and the stock panel's string `"0"` hides where Lua 5.0 holds it truthy.
            lua.create_function(move |lua, args: MultiValue| {
                let show = bool_or_default(args.front(), true);
                super::calls::submit(
                    lua,
                    super::ScriptCall::WornDisplay(WornDisplaySet { piece, show }),
                )
            })?,
        )?;
    }

    // `1` or nil off the live bit (`0x48dd8b`, `0x48ddeb`), nil with no player (`0x48dd80`); the
    // Options row feeds it straight into `SetChecked`.
    for (name, piece) in [
        ("ShowingHelm", WornDisplay::Helm),
        ("ShowingCloak", WornDisplay::Cloak),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let shown = read_self(lua, |own| match piece {
                    WornDisplay::Helm => own.shows_helm(),
                    WornDisplay::Cloak => own.shows_cloak(),
                });
                Ok(shown.then_some(1u32))
            })?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{WornDisplay, WornDisplaySet};
    use crate::script::UiScript;

    fn set(piece: WornDisplay, show: bool) -> WornDisplaySet {
        WornDisplaySet { piece, show }
    }

    /// The flag is `GetBoolOrDefault` with default 1 (`0x48de4e`): a bare call shows, a table
    /// takes the default, and a string goes by its first byte.
    #[test]
    fn the_setter_takes_the_flag_with_default_one() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        for (arg, show) in [
            ("", true),
            ("nil", false),
            ("{}", true),
            (r#""0""#, false),
            (r#""1""#, true),
            (r#""no""#, false),
            (r#""yes""#, true),
            (r#""off""#, false),
            (r#""""#, true),
            ("0.5", false),
            ("0", false),
            ("1", true),
        ] {
            s.run(&format!("ShowHelm({arg}) ShowCloak({arg})")).unwrap();
            assert_eq!(
                s.take_worn_display_sets(),
                [set(WornDisplay::Helm, show), set(WornDisplay::Cloak, show)],
                "ShowHelm({arg})"
            );
        }
    }

    /// Every call reaches the host, which alone reads the live bit (`0x5ee6b4`, `0x5ee773`): two
    /// hides before the server answers are two asks, not one.
    #[test]
    fn every_call_is_an_ask_and_the_getter_reads_the_wire_bit_not_the_ask() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("ShowHelm(0) ShowHelm(0) ShowHelm(1)").unwrap();
        assert_eq!(
            s.take_worn_display_sets(),
            [
                set(WornDisplay::Helm, false),
                set(WornDisplay::Helm, false),
                set(WornDisplay::Helm, true),
            ]
        );
        s.run("ShowHelm(0)").unwrap();
        assert!(
            s.eval::<bool>("return ShowingHelm() == 1").unwrap(),
            "the flag has not moved, so the getter still says shown"
        );
        s.set_worn_display(false, true);
        assert!(s.eval::<bool>("return ShowingHelm() == nil").unwrap());
        assert!(s.eval::<bool>("return ShowingCloak() == 1").unwrap());
    }
}
