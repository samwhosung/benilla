//! `SlashCmdList` dispatch from the host, for a line that reaches it without passing the stock
//! `ChatEdit_ParseText` (`ChatFrame.lua`): the same walk, over the table the stock `ChatFrame.lua`
//! declares, where the stock commands, benilla's layer's and an addon's are entries.

use mlua::{Table, Value};

impl super::UiScript {
    /// Dispatch `/cmd args` through `SlashCmdList`, returning whether a handler ran. The host calls
    /// this after its own commands miss, so an addon never shadows one; a handler that raises is
    /// recorded as a script error, not propagated, and still counts as handled.
    pub fn run_slash_command(&mut self, cmd: &str, args: &str) -> bool {
        let Some(handler) = self.find_slash_handler(cmd) else {
            return false;
        };
        // The rest of the line after the alias and one space, spaces intact: the reference's
        // `strsub(text, strlen(cmdString) + 2)`.
        if let Err(e) = handler.call::<()>(args.to_string()) {
            self.model_mut().record_script_error(format!("/{cmd}: {e}"));
        }
        true
    }

    /// Whether a `SlashCmdList` entry claims `cmd`, without running it.
    pub fn has_slash_command(&self, cmd: &str) -> bool {
        self.find_slash_handler(cmd).is_some()
    }

    /// Register a host command as a `SlashCmdList` row, as an addon registers one: `SLASH_<key><n>`
    /// for each alias (with its `/`) and a handler that queues `"<first alias> <msg>"` on the chat
    /// input the host drains ([`Self::take_chat_input`]). A no-op before `ChatFrame.lua` has
    /// declared `SlashCmdList`.
    pub fn register_host_slash_command(&self, key: &str, aliases: &[&str]) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let Ok(list) = globals.get::<Table>("SlashCmdList") else {
            return Ok(());
        };
        let Some(first) = aliases.first() else {
            return Ok(());
        };
        for (n, alias) in aliases.iter().enumerate() {
            globals.set(format!("SLASH_{key}{}", n + 1), *alias)?;
        }
        let first = first.to_string();
        let handler = self.lua.create_function(move |lua, msg: Option<String>| {
            let line = format!("{first} {}", msg.unwrap_or_default());
            lua.app_data_mut::<super::Model>()
                .expect("model app_data")
                .chat_input
                .push(line);
            Ok(())
        })?;
        list.set(key, handler)
    }

    /// The reference's walk: over `SlashCmdList`'s keys, then `SLASH_<key><n>` to the first gap,
    /// case-insensitive.
    fn find_slash_handler(&self, cmd: &str) -> Option<mlua::Function> {
        let globals = self.lua.globals();
        let list: Table = globals.get("SlashCmdList").ok()?;
        let want = cmd.to_ascii_uppercase();
        for pair in list.pairs::<String, Value>() {
            let Ok((index, Value::Function(handler))) = pair else {
                continue; // Deviation: skipped because it is no command; the reference raises
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

    #[test]
    fn an_addon_registers_a_slash_command_and_it_dispatches() {
        let mut s = UiScript::new().unwrap();
        // A bare VM has no `ChatFrame.xml`, so the test declares the table itself.
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

    /// The reference's `while cmdString` loop.
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

    /// A host row is an ordinary `SlashCmdList` row that queues its line for the host.
    #[test]
    fn a_host_command_queues_its_line() {
        let mut s = UiScript::new().unwrap();
        s.register_host_slash_command("HOSTPROBE", &["/hostprobe", "/hp"])
            .unwrap();
        assert!(
            !s.has_slash_command("hostprobe"),
            "no table yet: nothing registers"
        );
        s.run("SlashCmdList = {}").unwrap();
        s.register_host_slash_command("HOSTPROBE", &["/hostprobe", "/hp"])
            .unwrap();
        assert_eq!(s.eval::<String>("return SLASH_HOSTPROBE2").unwrap(), "/hp");
        assert!(s.run_slash_command("hp", "a b"));
        assert_eq!(s.take_chat_input(), ["/hostprobe a b"]);
    }

    #[test]
    fn a_non_function_entry_is_not_a_command() {
        let s = UiScript::new().unwrap();
        s.run("SlashCmdList = {}").unwrap();
        s.run(r#"SLASH_ODD1 = "/odd" SlashCmdList["ODD"] = "not a function""#)
            .unwrap();
        assert!(!s.has_slash_command("odd"));
    }
}
