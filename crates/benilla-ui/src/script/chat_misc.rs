//! The chat slash handlers' engine verbs that are not sends, channels, or window state —
//! `DoEmote`, `RandomRoll`, `AssistByName`, `UninviteByName`, `ConsoleExec`, `LoggingChat`,
//! `LoggingCombat`. Each is what a stock `ChatFrame.lua` built-in handler calls once it has
//! parsed the line (`SlashCmdList` walk); the app used to parse these lines
//! itself (`ui_chat/input/parse.rs`) and now drains what the reference's own Lua decided.
//!
//! Every verb is a queue or a flag — the engine-free seam: the VM never sees the wire,
//! the app drains and sends. Registrar addresses:
//! `0x49fd30 DoEmote`, `0x48c7b0 RandomRoll`, `0x489c40 AssistByName`, `0x48a610 UninviteByName`.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// `DoEmote(token [, target])` — the `EmotesText.dbc` **name token** (`"WAVE"`, not `/wave`) and
/// the optional target name the handler split off the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmoteRequest {
    pub token: String,
    pub target: Option<String>,
}

/// Lua number coercion the way the reference's `lua_tonumber` does it: an integer, a float, or a
/// string that parses; anything else is 0.
fn to_u32(v: &Value) -> u32 {
    match v {
        Value::Integer(i) => u32::try_from(*i).unwrap_or(0),
        Value::Number(n) if n.is_finite() && *n >= 0.0 => *n as u32,
        Value::String(s) => s
            .to_str()
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map_or(0, |n| n as u32),
        _ => 0,
    }
}

fn flag_arg(v: Option<Value>) -> Option<bool> {
    match v {
        None | Some(Value::Nil) => None,
        Some(Value::Boolean(b)) => Some(b),
        Some(Value::Integer(n)) => Some(n != 0),
        Some(Value::Number(n)) => Some(n != 0.0),
        Some(Value::String(s)) => Some(s.to_str().is_ok_and(|s| s != "0" && !s.is_empty())),
        Some(_) => Some(true),
    }
}

impl super::UiScript {
    /// `DoEmote` calls since the last drain.
    pub fn take_emote_requests(&mut self) -> Vec<EmoteRequest> {
        std::mem::take(&mut self.model_mut().emote_requests)
    }

    /// `RandomRoll(min, max)` calls since the last drain, as the app sends them
    /// (`CMSG_RANDOM_ROLL`).
    pub fn take_roll_requests(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().roll_requests)
    }

    /// `UninviteByName` calls since the last drain.
    pub fn take_uninvite_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().uninvite_requests)
    }

    /// `ConsoleExec` lines whose first word is not a registered CVar — the console commands the
    /// app owns (`reloadui`, …). A CVar line is written to the store directly and never appears
    /// here.
    pub fn take_console_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().console_lines)
    }

    /// `(chat, combat)` — the two logging flags as `LoggingChat`/`LoggingCombat` last set them.
    pub fn logging_flags(&self) -> (bool, bool) {
        let model = self.model_mut();
        (model.logging_chat, model.logging_combat)
    }

    /// Whether either logging flag moved since the last drain — the app's cue to open or close
    /// its log files.
    pub fn take_logging_changes(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().logging_changed)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "DoEmote",
        lua.create_function(|lua, (token, target): (Option<String>, Option<String>)| {
            let Some(token) = token.filter(|t| !t.is_empty()) else {
                return Ok(());
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.emote_requests.push(EmoteRequest {
                token: token.to_ascii_uppercase(),
                target: target.filter(|t| !t.trim().is_empty()),
            });
            Ok(())
        })?,
    )?;

    g.set(
        "RandomRoll",
        lua.create_function(|lua, (min, max): (Value, Value)| {
            let (min, max) = (to_u32(&min), to_u32(&max));
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.roll_requests.push((min, max));
            Ok(())
        })?,
    )?;

    g.set(
        "AssistByName",
        lua.create_function(|lua, name: Option<String>| {
            let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
                return Ok(());
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .selection_requests
                .push(super::SelectionRequest::AssistByName(name));
            Ok(())
        })?,
    )?;

    g.set(
        "UninviteByName",
        lua.create_function(|lua, name: Option<String>| {
            let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
                return Ok(());
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.uninvite_requests.push(name);
            Ok(())
        })?,
    )?;

    // ConsoleExec("name value") — the console line `/console` hands over. A registered CVar
    // WITH a value is written the way `SetCVar` writes it (same store, same change queue, no
    // CVAR_UPDATE token), synchronously, so a script reading it back on the next line sees the
    // write. Everything else — a command name, or a bare CVar name, which the reference's
    // per-CVar console command answers with `CVar "%s" is "%s"` (`0x63dde0`) — is a line for
    // the host's command registry.
    g.set(
        "ConsoleExec",
        lua.create_function(|lua, line: Option<String>| {
            let Some(line) = line.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()) else {
                return Ok(());
            };
            let (name, value) = line
                .split_once(char::is_whitespace)
                .map_or((line.as_str(), ""), |(n, v)| (n, v.trim()));
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let is_cvar = model.cvars.contains_key(&name.to_ascii_lowercase());
            if is_cvar && !value.is_empty() {
                super::cvars::write_cvar(&mut model, name, value.to_string(), None);
            } else {
                model.console_lines.push(line.clone());
            }
            Ok(())
        })?,
    )?;

    for (name, chat) in [("LoggingChat", true), ("LoggingCombat", false)] {
        g.set(
            name,
            lua.create_function(move |lua, flag: Option<Value>| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let slot = if chat {
                    &mut model.logging_chat
                } else {
                    &mut model.logging_combat
                };
                match flag_arg(flag) {
                    None => Ok(MultiValue::from_vec(vec![if *slot {
                        Value::Integer(1)
                    } else {
                        Value::Nil
                    }])),
                    Some(on) => {
                        if *slot != on {
                            *slot = on;
                            model.logging_changed = true;
                        }
                        Ok(MultiValue::new())
                    }
                }
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

    #[test]
    fn random_roll_coerces_the_strings_the_stock_handler_passes() {
        let mut s = UiScript::new().unwrap();
        s.run("RandomRoll('1', '100') RandomRoll(5, 10) RandomRoll('x', nil)")
            .unwrap();
        assert_eq!(s.take_roll_requests(), vec![(1, 100), (5, 10), (0, 0)]);
    }

    #[test]
    fn assist_and_uninvite_by_name_queue_their_names() {
        let mut s = UiScript::new().unwrap();
        s.run("AssistByName('Alice') AssistByName('') UninviteByName('Carol') UninviteByName()")
            .unwrap();
        assert_eq!(
            s.take_selection_requests(),
            vec![SelectionRequest::AssistByName("Alice".into())]
        );
        assert_eq!(s.take_uninvite_requests(), vec!["Carol".to_string()]);
    }

    #[test]
    fn console_exec_writes_a_registered_cvar_and_queues_anything_else() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("uiScale", "1")]);
        s.run("ConsoleExec('uiscale 0.8') ConsoleExec('reloadui') ConsoleExec('  ') ConsoleExec('uiScale')")
            .unwrap();
        assert_eq!(
            s.eval::<String>("return GetCVar('uiScale')").unwrap(),
            "0.8",
            "the CVar store is the same one SetCVar writes, matched case-insensitively"
        );
        // A bare CVar name is the host's to answer (2303: the reference prints its value from
        // the per-CVar console command), so it rides the command lane like `reloadui`.
        assert_eq!(
            s.take_console_lines(),
            vec!["reloadui".to_string(), "uiScale".to_string()]
        );
        assert_eq!(
            s.take_cvar_changes(),
            vec![("uiScale".to_string(), "0.8".to_string())],
            "a bare name writes nothing"
        );
    }

    #[test]
    fn the_logging_flags_read_as_one_or_nil_and_set_from_any_truthy_arg() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return LoggingChat() == nil").unwrap());
        assert!(s.eval::<bool>("return LoggingCombat() == nil").unwrap());
        assert_eq!(
            s.arity("LoggingChat(true)").unwrap(),
            0,
            "the setter returns nothing"
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
    }
}
