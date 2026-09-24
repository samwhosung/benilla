//! **`SlashCmdList` dispatch** — how an addon gets a `/command`.
//!
//! The single most-wanted *table* in the corpus. 26 of 218 addons stop on
//! `attempt to index global 'SlashCmdList'` — the top runtime wall after the dialect gap — because
//! the registration idiom is three lines at file scope:
//!
//! ```lua
//! SLASH_MYADDON1 = "/myaddon"
//! SLASH_MYADDON2 = "/ma"
//! SlashCmdList["MYADDON"] = function(msg) … end
//! ```
//!
//! …and the third line raises the moment the table is missing.
//!
//! ## The resolution is the reference's own, transcribed
//!
//! `ChatEdit_ParseText` (ChatFrame.lua) walks **`SlashCmdList`'s keys**, not a name list: for each
//! `index` it reads `SLASH_<index>1`, `SLASH_<index>2`, … until the first gap, comparing each
//! against the typed command case-insensitively. The aliases are data and the handlers are code,
//! joined by the index name — decision 0881 built our own table on exactly that shape, and this is
//! the same walk over the half addons own.
//!
//! Two details of that walk are load-bearing and both are easy to get wrong:
//!
//! - **The handler's argument is the rest of the line, not the rest of the *word*.** The reference
//!   passes `strsub(text, strlen(cmdString) + 2)` — everything after the alias and one space — so
//!   `/myaddon set foo 3` hands the handler `"set foo 3"`, spaces intact. An addon's own parser
//!   starts there.
//! - **The walk stops at the first gap.** `SLASH_X1` and `SLASH_X3` with no `SLASH_X2` means `X3`
//!   is unreachable, in the reference and here. That is a property of the data format, not a bug
//!   to repair.
//!
//! ## Where the table lives, and the divergence that comes with it
//!
//! `SlashCmdList` is **FrameXML's**, not the engine's (`reference/1.12-globals.tsv` says
//! `table framexml`), so ours is declared in our transcribed `ChatFrame.xml` exactly as the
//! reference declares it in `ChatFrame.lua` — 1190's engine/FrameXML split applied to a table
//! rather than a function.
//!
//! **The divergence, stated:** benilla's *own* commands (`/who`, `/invite`, `/cast`, …) are
//! dispatched in Rust ([`crate`]'s host owns them) and are **not** entries in `SlashCmdList`. In
//! the reference they are, so an addon that tests `if SlashCmdList["WHO"] then` or hooks one by
//! wrapping the entry sees an empty table here and does nothing. That is a real, observable gap;
//! it is bounded (nothing in the corpus does it) and the fix is to move our command handlers into
//! transcribed FrameXML, which is a much larger piece of work than this one.

use mlua::{Table, Value};

impl super::UiScript {
    /// Try to dispatch `/cmd args` through `SlashCmdList`. Returns whether a handler ran.
    ///
    /// The host calls this **after** its own command table misses, so a shipped command can never
    /// be shadowed by an addon — which is also the reference's precedence, since its own commands
    /// are registered in the same table before any addon loads.
    ///
    /// A handler that raises is collected into [`super::UiScript::errors`] like any other script
    /// error rather than propagated: a broken `/command` must not take down the chat drain that
    /// invoked it. It still counts as *handled* — the command existed and was called.
    pub fn run_slash_command(&mut self, cmd: &str, args: &str) -> bool {
        let Some(handler) = self.find_slash_handler(cmd) else {
            return false;
        };
        // The reference passes `strsub(text, strlen(cmdString) + 2)` — everything after the alias
        // and one space. The host has already made that split, so `args` IS that string.
        if let Err(e) = handler.call::<()>(args.to_string()) {
            self.model_mut().record_script_error(format!("/{cmd}: {e}"));
        }
        true
    }

    /// Is `cmd` a command some `SlashCmdList` entry claims? Pure query, fires nothing — the
    /// completion/echo paths want to know without running anything.
    pub fn has_slash_command(&self, cmd: &str) -> bool {
        self.find_slash_handler(cmd).is_some()
    }

    /// The reference's walk: over `SlashCmdList`'s keys, then `SLASH_<key><n>` until a gap.
    fn find_slash_handler(&self, cmd: &str) -> Option<mlua::Function> {
        let globals = self.lua.globals();
        let list: Table = globals.get("SlashCmdList").ok()?;
        let want = cmd.to_ascii_uppercase();
        for pair in list.pairs::<String, Value>() {
            let Ok((index, Value::Function(handler))) = pair else {
                continue; // a non-function entry is not a command; the reference would error on it
            };
            for n in 1.. {
                let alias: Option<String> = globals.get(format!("SLASH_{index}{n}")).ok().flatten();
                let Some(alias) = alias else { break };
                let alias = alias.trim_start_matches('/');
                if alias.eq_ignore_ascii_case(&want) {
                    return Some(handler);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// **The registration idiom, exactly as an addon writes it** — and the reason 26 corpus addons
    /// died at file scope before this landed.
    #[test]
    fn an_addon_registers_a_slash_command_and_it_dispatches() {
        let mut s = UiScript::new().unwrap();
        // Our shipped ChatFrame.xml declares the table; a bare VM has to stand it up itself, which
        // is also the smallest possible statement of what an addon depends on.
        s.run("SlashCmdList = {}").unwrap();
        s.run(
            r#"
            SLASH_PROBE1 = "/probe"
            SLASH_PROBE2 = "/pr"
            SlashCmdList["PROBE"] = function(msg) ProbeGot = msg end
            "#,
        )
        .unwrap();

        assert!(s.run_slash_command("probe", "set foo 3"));
        assert_eq!(
            s.eval::<String>("return ProbeGot").unwrap(),
            "set foo 3",
            "the handler gets the rest of the LINE, spaces intact — its own parser starts there"
        );

        // The second alias reaches the same handler, and matching is case-insensitive.
        assert!(s.run_slash_command("PR", "again"));
        assert_eq!(s.eval::<String>("return ProbeGot").unwrap(), "again");

        // An unclaimed command is not handled, so the host still prints HELP_TEXT_SIMPLE.
        assert!(!s.run_slash_command("nosuchthing", ""));
    }

    /// The alias walk stops at the first gap — the reference's `while cmdString` loop, and a
    /// property of the data format rather than a bug to repair.
    #[test]
    fn the_alias_walk_stops_at_the_first_gap() {
        let s = UiScript::new().unwrap();
        s.run("SlashCmdList = {}").unwrap();
        s.run(
            r#"
            SLASH_GAPPY1 = "/one"
            SLASH_GAPPY3 = "/three"
            SlashCmdList["GAPPY"] = function() end
            "#,
        )
        .unwrap();
        assert!(s.has_slash_command("one"));
        assert!(
            !s.has_slash_command("three"),
            "SLASH_GAPPY3 is unreachable without SLASH_GAPPY2 — in the reference too"
        );
    }

    /// A handler that raises is collected, not propagated: a broken addon command must not take
    /// down the chat drain that invoked it.
    #[test]
    fn a_raising_handler_is_collected_and_still_counts_as_handled() {
        let mut s = UiScript::new().unwrap();
        s.run("SlashCmdList = {}").unwrap();
        s.run(
            r#"
            SLASH_BOOM1 = "/boom"
            SlashCmdList["BOOM"] = function() error("nope") end
            "#,
        )
        .unwrap();
        assert!(
            s.run_slash_command("boom", ""),
            "the command existed and ran — that it failed is a separate fact"
        );
        assert!(
            s.errors().iter().any(|e| e.contains("/boom")),
            "the failure surfaces where every other script error does"
        );
    }

    /// A `SlashCmdList` entry that is not a function is skipped rather than called.
    #[test]
    fn a_non_function_entry_is_not_a_command() {
        let s = UiScript::new().unwrap();
        s.run("SlashCmdList = {}").unwrap();
        s.run(r#"SLASH_ODD1 = "/odd" SlashCmdList["ODD"] = "not a function""#)
            .unwrap();
        assert!(!s.has_slash_command("odd"));
    }
}
