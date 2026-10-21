//! The engine verbs stock `ChatFrame.lua`'s slash handlers call that are not sends, channels or
//! window state: `DoEmote` (`0x49fd30`), `RandomRoll` (`0x48c7b0`), `AssistByName` (`0x489c40`),
//! `UninviteByName` (`0x48a610`), `LoggingChat` and `LoggingCombat`. The first four apply inside
//! the call ([`super::ScriptCall`]); the logging pair is a flag the app drains.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::bool_or_default;
use super::Model;

/// `DoEmote(token [, target])`: the `EmotesText.dbc` name token (`"WAVE"`, not `/wave`) and an
/// optional target string (`0x49fd30` reads arg 2 with `lua_tostring`): a player name, or a `%`
/// word for the selection. A non-string, non-number second argument reads as none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmoteRequest {
    pub token: String,
    pub target: Option<String>,
}

impl super::UiScript {
    /// The `DoEmote` calls a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_emote_requests(&mut self) -> Vec<EmoteRequest> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::DoEmote(e) => Some(e.clone()),
            _ => None,
        })
    }

    /// The `RandomRoll(min, max)` sends a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_roll_requests(&mut self) -> Vec<(u32, u32)> {
        self.take_calls_where(|call| match *call {
            super::ScriptCall::RandomRoll { min, max } => Some((min, max)),
            _ => None,
        })
    }

    /// The `UninviteByName` calls a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_uninvite_requests(&mut self) -> Vec<String> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Uninvite(name) => Some(name.clone()),
            _ => None,
        })
    }

    /// `ConsoleExec` lines its own CVar commands did not run ([`super::console_exec`]): the app's
    /// console commands (`reloadui`, …), a bare CVar name, an unknown word.
    pub fn take_console_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().console_lines)
    }

    /// `(chat, combat)`, as `LoggingChat` and `LoggingCombat` last set them.
    pub fn logging_flags(&self) -> (bool, bool) {
        let model = self.model_mut();
        (model.logging_chat, model.logging_combat)
    }

    /// Whether a logging flag moved since the last drain: the app's cue to open or close its logs.
    pub fn take_logging_changes(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().logging_changed)
    }
}

/// `Usage: RandomRoll("max") or RandomRoll("min", "max")`, `0x842a94`.
const USAGE_RANDOM_ROLL: &str = r#"Usage: RandomRoll("max") or RandomRoll("min", "max")"#;

/// `cmp edi,0xf4240` (`0x48c82a`): the highest `max` that rolls.
const MAX_ROLL: i32 = 1_000_000;

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "DoEmote",
        lua.create_function(|lua, (token, target): (Option<String>, Value)| {
            let Some(token) = token.filter(|t| !t.is_empty()) else {
                return Ok(());
            };
            let request = EmoteRequest {
                token: token.to_ascii_uppercase(),
                // `lua_tostring(2)` (`0x49fdc5`): a number is its `%.14g` text.
                target: crate::script::binding_abi::optional_string(lua, &target)
                    .filter(|t| !t.is_empty()),
            };
            super::calls::submit(lua, super::ScriptCall::DoEmote(request))
        })?,
    )?;

    g.set(
        "RandomRoll",
        lua.create_function(|lua, (min, max): (Value, Value)| {
            // Both must be a string or a number (`lua_isstring`, `0x48c7c0`, `0x48c7d4`), else the
            // usage error (`0x842a94`, at `0x48c8a1`); each is read with `SStrToInt` (`0x64ac60`).
            let min = super::binding_abi::string_arg(lua, min, USAGE_RANDOM_ROLL)?;
            let max = super::binding_abi::string_arg(lua, max, USAGE_RANDOM_ROLL)?;
            let (min, max) = (
                super::binding_abi::s_str_to_int(&min),
                super::binding_abi::s_str_to_int(&max),
            );
            // `0x48c807`-`0x48c830`: nothing for `(0, 0)`, a negative `min`, `max` below `min` or
            // `max` past 1000000, compared signed.
            if (min == 0 && max == 0) || min < 0 || max < min || max > MAX_ROLL {
                return Ok(());
            }
            super::calls::submit(
                lua,
                super::ScriptCall::RandomRoll {
                    min: min as u32,
                    max: max as u32,
                },
            )
        })?,
    )?;

    // AssistByName(name) (`0x489c40`): a non-string raises `Usage:` (`0x489d3f`); any string,
    // the empty one included, goes to the host, which assists the named player or, for an empty
    // name, the selection (`0x489c89`), and shows the line its miss owes (`0x489d17`, `0x489d2b`).
    g.set(
        "AssistByName",
        lua.create_function(|lua, name: Value| {
            let name = super::binding_abi::string_arg(lua, name, r#"Usage: AssistByName("name")"#)?;
            let may_cast = super::protected::select_may_cast(lua);
            super::calls::submit(
                lua,
                super::ScriptCall::Select {
                    request: super::SelectionRequest::AssistByName(name),
                    may_cast,
                },
            )?;
            Ok(())
        })?,
    )?;

    // UninviteByName(name) (`0x48a610`): `lua_isstring` (`0x6f3510` at `0x48a61e`) else
    // `Usage: UninviteByName("name")` (`0x48a698`, the string at `0x8427c0`); any string, the
    // empty one included, goes into `CMSG_GROUP_UNINVITE` as it is (`0x48a65e`), with no trim.
    g.set(
        "UninviteByName",
        lua.create_function(|lua, name: Value| {
            let name =
                super::binding_abi::string_arg(lua, name, r#"Usage: UninviteByName("name")"#)?;
            super::calls::submit(lua, super::ScriptCall::Uninvite(name))
        })?,
    )?;

    for (name, chat) in [("LoggingChat", true), ("LoggingCombat", false)] {
        g.set(
            name,
            lua.create_function(move |lua, args: MultiValue| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                // With an argument (`lua_gettop > 0`, `0x49fe57`) the flag is set from
                // `GetBoolOrDefault` (`0x49fe69`, default 1), so an explicit nil clears it. Both
                // paths then answer the flag as it stands, 1 or nil, one value (`0x49feb2`-`0x49fee1`):
                // the reference re-reads it after the log-open attempt, which can clear it
                // (`0x49feab`); the app owns that attempt here.
                if let Some(on) = args.front().map(|v| bool_or_default(Some(v), true)) {
                    let slot = if chat {
                        &mut model.logging_chat
                    } else {
                        &mut model.logging_combat
                    };
                    if *slot != on {
                        *slot = on;
                        model.logging_changed = true;
                    }
                }
                let now = if chat {
                    model.logging_chat
                } else {
                    model.logging_combat
                };
                Ok(if now { Value::Integer(1) } else { Value::Nil })
            })?,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{SelectionRequest, UiScript};

    #[test]
    fn do_emote_queues_the_uppercased_token_and_a_non_empty_target() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("DoEmote('wave', 'Bob') DoEmote('DANCE', '') DoEmote('') DoEmote()")
            .unwrap();
        assert_eq!(
            s.take_emote_requests(),
            vec![
                EmoteRequest {
                    token: "WAVE".into(),
                    target: Some("Bob".into())
                },
                EmoteRequest {
                    token: "DANCE".into(),
                    target: None
                },
            ]
        );
        assert!(s.take_emote_requests().is_empty(), "drained");
    }

    /// The second argument is a string, never a unit token: `"target"` is a name like any other,
    /// and `%t` reaches the app's selection arm untouched (`0x49fdb1`).
    #[test]
    fn do_emote_second_argument_is_passed_as_a_string() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("DoEmote('WAVE', 'target') DoEmote('WAVE', '%t') DoEmote('WAVE', {})")
            .unwrap();
        let targets: Vec<_> = s
            .take_emote_requests()
            .into_iter()
            .map(|e| e.target)
            .collect();
        assert_eq!(
            targets,
            vec![Some("target".into()), Some("%t".into()), None]
        );
    }

    /// `RandomRoll` reads both arguments as text (`0x48c7c0`, `0x48c7d4`) through `SStrToInt`
    /// (`0x64ac60`), so a number is its text and `"12x"` is 12.
    #[test]
    fn random_roll_reads_the_strings_the_stock_handler_passes() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run(
            "RandomRoll('1', '100') RandomRoll(5, 10) RandomRoll('1', '12x') RandomRoll(' 1', 2)",
        )
        .unwrap();
        assert_eq!(
            s.take_roll_requests(),
            vec![(1, 100), (5, 10), (1, 12), (0, 2)]
        );
    }

    /// Both arguments must be a string or a number, else the usage error (`0x842a94`, at
    /// `0x48c8a1`): `RandomRoll("100")` raises, though its usage text names that form.
    #[test]
    fn random_roll_raises_its_usage_without_two_strings() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        for bad in [
            "RandomRoll()",
            "RandomRoll('100')",
            "RandomRoll(nil, 100)",
            "RandomRoll(1, {})",
            "RandomRoll(true, 2)",
        ] {
            let err = s.run(bad).expect_err(bad).to_string();
            assert!(
                err.contains(r#"Usage: RandomRoll("max") or RandomRoll("min", "max")"#),
                "{bad}: {err}"
            );
        }
        assert!(s.take_roll_requests().is_empty());
    }

    /// `0x48c807`-`0x48c830`: no roll for `(0, 0)`, a negative `min`, `max` below `min` or `max`
    /// past 1000000; `(0, max)` and `(n, n)` roll.
    #[test]
    fn random_roll_sends_only_an_ordered_range_up_to_a_million() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run(
            "RandomRoll(0, 0) RandomRoll(-1, 5) RandomRoll(10, 5) RandomRoll(1, 1000001) \
             RandomRoll('x', 'y') RandomRoll('1', 'x') RandomRoll(0, 1000000) RandomRoll(7, 7) \
             RandomRoll('x', 3)",
        )
        .unwrap();
        assert_eq!(s.take_roll_requests(), vec![(0, 1_000_000), (7, 7), (0, 3)]);
    }

    /// `AssistByName` hands the host any string, the empty one included, which assists the
    /// selection (`0x489c89`).
    #[test]
    fn assist_and_uninvite_by_name_queue_their_names() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("AssistByName('Alice') AssistByName('') UninviteByName('Carol')")
            .unwrap();
        assert_eq!(
            s.take_selection_requests(),
            vec![
                SelectionRequest::AssistByName("Alice".into()),
                SelectionRequest::AssistByName(String::new()),
            ]
        );
        assert_eq!(s.take_uninvite_requests(), vec!["Carol".to_string()]);
    }

    /// `UninviteByName` (`0x48a610`) tests its argument with `lua_isstring` (`0x6f3510` at
    /// `0x48a61e`): a non-string, nil and no argument included, raises `Usage:
    /// UninviteByName("name")` (`0x48a625` → `0x48a698`, the string at `0x8427c0`). Any string,
    /// a number's text among them, goes into `CMSG_GROUP_UNINVITE` as it is (`0x48a655`,
    /// `0x48a65e`) and is sent (`0x48a669`): no trim and no empty test.
    #[test]
    fn uninvite_by_name_raises_for_a_non_string_and_sends_any_string() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        for arg in ["", "nil", "true", "{}"] {
            let e = s
                .run(&format!("UninviteByName({arg})"))
                .expect_err(arg)
                .to_string();
            assert!(e.contains(r#"Usage: UninviteByName("name")"#), "{arg}: {e}");
        }
        s.run("UninviteByName('') UninviteByName('  ') UninviteByName(5) UninviteByName('Carol')")
            .unwrap();
        assert_eq!(s.take_uninvite_requests(), ["", "  ", "5", "Carol"]);
    }

    #[test]
    fn the_logging_flags_read_as_one_or_nil_and_set_from_any_truthy_arg() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return LoggingChat() == nil").unwrap());
        assert!(s.eval::<bool>("return LoggingCombat() == nil").unwrap());
        assert_eq!(
            s.arity("LoggingChat(true)").unwrap(),
            1,
            "the setter answers the flag too (`0x49fed3`)"
        );
        assert_eq!(s.eval::<i64>("return LoggingChat()").unwrap(), 1);
        assert_eq!(s.logging_flags(), (true, false));
        assert!(s.take_logging_changes());
        s.run("LoggingCombat(1) LoggingChat(false)").unwrap();
        assert_eq!(s.logging_flags(), (false, true));
        assert!(s.take_logging_changes());
        s.run("LoggingCombat(true)").unwrap();
        assert!(
            !s.take_logging_changes(),
            "a write that moves nothing cues nothing"
        );
        // The setter answers the flag it leaves: 1 or nil, one value either way.
        assert_eq!(s.eval::<i64>("return LoggingChat(true)").unwrap(), 1);
        assert_eq!(s.arity("LoggingChat(nil)").unwrap(), 1);
        assert!(s.eval::<bool>("return LoggingChat(nil) == nil").unwrap());
    }

    /// `AssistByName` (`0x489c40`) selects inside its call with a host lent.
    #[test]
    fn assist_by_name_applies_in_the_call_with_a_host_lent() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            crate::script::host::test_support::applied_in_call(&mut s, "AssistByName('Alice')"),
            vec![crate::script::ScriptCall::Select {
                request: SelectionRequest::AssistByName("Alice".into()),
                may_cast: true,
            }]
        );
    }
}
