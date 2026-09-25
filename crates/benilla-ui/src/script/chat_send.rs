//! `SendChatMessage(text [, chatType [, language [, target]]])` (`0x49f1e0`): the engine verb an
//! addon speaks through, with no FrameXML between it and the wire. `chatType` defaults to
//! `"SAY"`; the fourth argument is the whisper target or the channel's number.
//!
//! It queues a [`ChatSend`] rather than going through the edit box, whose drain runs the slash
//! grammar: an addon's `SendChatMessage("/dance")` says the characters. The reference splits the
//! same way, `ChatEdit_SendText` parsing and `SendChatMessage` not.
//!
//! `language` is accepted and dropped, so every line goes in the speaker's default tongue: the
//! app's chat command has no language field. The reference sends the named language.
//!
//! Deviation: no truncation at 255 characters (`0x49f604`), because vmangos refuses a longer line
//! itself (`Chat.cpp:2177`): it is dropped where the reference sends its first 255. The server
//! throttles in both.

use mlua::{Lua, MultiValue, Value};

use super::Model;

impl super::UiScript {
    /// Push the name `GetDefaultLanguage()` answers. `None` is the reference's no-player state,
    /// which returns zero values, not `nil`; the app resolves it per world entry from
    /// `ChrRaces.BaseLanguage` and `Languages.dbc`.
    pub fn set_default_language(&mut self, name: Option<String>) {
        self.model_mut().default_language = name;
    }
}

/// One queued `SendChatMessage` line, plain data: this crate holds no wire types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSend {
    /// The line, verbatim: a leading `/` is text, not a command.
    pub text: String,
    /// The chat type token (`"SAY"`, `"WHISPER"`, `"CHANNEL"`, …), uppercased, since the
    /// reference's compare ignores case.
    pub chat_type: String,
    /// The fourth argument as `lua_tostring` gives it (`0x49f306`): the whisper target, or the
    /// channel number the app resolves to a joined channel's name.
    pub target: Option<String>,
}

impl super::UiScript {
    /// The languages this character knows, in `Languages.dbc` row order: a language counts when a
    /// known spell with `Effect_1 == 39` teaches it (`0x4b25b0`) and its skill line is in the skill
    /// block (`0x5ec720`). A change fires `LANGUAGE_LIST_CHANGED` (`0x49b970`, event 0x102).
    pub fn set_known_languages(&mut self, names: Vec<String>) {
        let changed = {
            let mut model = self.model_mut();
            if model.known_languages == names {
                false
            } else {
                model.known_languages = names;
                true
            }
        };
        if changed {
            self.fire_event("LANGUAGE_LIST_CHANGED", vec![]);
        }
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // GetNumLaguages(), `0x49fb30`: one number, the known languages. The misspelling is the
    // reference's registered name (`0x843628`); the correct spelling is nowhere in the binary.
    lua.globals().set(
        "GetNumLaguages",
        lua.create_function(|lua, _ignored: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.known_languages.len() as i64)
        })?,
    )?;
    // GetLanguageByIndex(i), `0x49fbe0`: the `Name_lang` of the i-th known language, positional,
    // never a row number or a language id; past the end it pushes nothing.
    lua.globals().set(
        "GetLanguageByIndex",
        lua.create_function(|lua, index: Option<i64>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let name = index
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.known_languages.get(i));
            Ok(match name {
                Some(n) => MultiValue::from_vec(vec![Value::String(lua.create_string(n)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;
    // GetDefaultLanguage(), `0x49fcd0`: one string (`0x6f3890`), or zero values, never `nil`, on
    // all four failure edges (`0x49fd2a`). It reads no arguments, so an addon's
    // `GetDefaultLanguage("player")` is ignored.
    lua.globals().set(
        "GetDefaultLanguage",
        lua.create_function(|lua, _ignored: MultiValue| {
            let name = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.default_language.clone()
            };
            Ok(match name {
                Some(n) => MultiValue::from_vec(vec![Value::String(lua.create_string(&n)?)]),
                None => MultiValue::new(),
            })
        })?,
    )?;

    lua.globals().set(
        "SendChatMessage",
        lua.create_function(
            |lua, (text, chat_type, _language, target): (String, Option<String>, Value, Value)| {
                let chat_type = chat_type
                    .unwrap_or_else(|| "SAY".into())
                    .to_ascii_uppercase();
                // `lua_isstring` then `lua_tostring` (`0x49f2f6`, `0x49f306`): a number is its
                // text, so `2.7` reaches the channel's `SStrToInt` as "2.7", which reads 2.
                let target = super::binding_abi::optional_string(lua, &target);
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .chat_sends
                    .push(ChatSend {
                        text,
                        chat_type,
                        target,
                    });
                Ok(())
            },
        )?,
    )?;
    Ok(())
}

impl super::UiScript {
    /// Drain the lines `SendChatMessage` queued since the last call.
    pub fn take_chat_sends(&mut self) -> Vec<ChatSend> {
        std::mem::take(&mut self.model_mut().chat_sends)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    #[test]
    fn send_chat_message_queues_a_line_without_parsing_it() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendChatMessage("hello")"#).unwrap();
        s.run(r#"SendChatMessage("/dance", "say")"#).unwrap();
        assert_eq!(
            s.take_chat_sends(),
            vec![
                ChatSend {
                    text: "hello".into(),
                    chat_type: "SAY".into(),
                    target: None,
                },
                ChatSend {
                    text: "/dance".into(),
                    chat_type: "SAY".into(),
                    target: None,
                },
            ]
        );
        assert!(s.take_chat_sends().is_empty());
    }

    #[test]
    fn the_target_argument_is_its_lua_tostring_text() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendChatMessage("hi", "WHISPER", nil, "Bob")"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, 1)"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, "General")"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, 2.7)"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, {})"#)
            .unwrap();
        let sent = s.take_chat_sends();
        assert_eq!(sent[0].target.as_deref(), Some("Bob"));
        assert_eq!(sent[1].target.as_deref(), Some("1"));
        assert_eq!(sent[2].target.as_deref(), Some("General"));
        assert_eq!(
            sent[3].target.as_deref(),
            Some("2.7"),
            "not rounded: the app's `SStrToInt` reads 2"
        );
        assert_eq!(sent[4].target, None, "a table is no string");
        assert_eq!(sent[1].chat_type, "CHANNEL");
    }

    #[test]
    fn the_language_argument_is_accepted_and_ignored() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendChatMessage("hi", "SAY", 7)"#).unwrap();
        assert_eq!(s.take_chat_sends().len(), 1);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// The count matters: an addon passes the result straight to `SendChatMessage` as `language`.
    #[test]
    fn get_default_language_is_one_string_or_zero_values() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("GetDefaultLanguage()").unwrap(),
            0,
            "no player object → ZERO values, not nil"
        );

        s.set_default_language(Some("Common".into()));
        assert_eq!(
            s.arity("GetDefaultLanguage()").unwrap(),
            1,
            "one value — not (name, id)"
        );
        assert_eq!(
            s.eval::<String>("return GetDefaultLanguage()").unwrap(),
            "Common"
        );
        // The binding reads no arguments; addons pass `"player"` anyway.
        assert_eq!(
            s.eval::<String>(r#"return GetDefaultLanguage("player")"#)
                .unwrap(),
            "Common"
        );
    }
}

#[cfg(test)]
mod language_tests {
    use crate::script::UiScript;

    #[test]
    fn the_language_walk_is_positional_over_the_known_rows() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumLaguages()").unwrap(), 0);
        assert_eq!(
            s.arity("GetLanguageByIndex(1)").unwrap(),
            0,
            "past the end pushes nothing, not nil"
        );
        s.run(
            "local f = CreateFrame('Frame') f:RegisterEvent('LANGUAGE_LIST_CHANGED') \
             f:SetScript('OnEvent', function() FIRED = (FIRED or 0) + 1 end)",
        )
        .unwrap();
        s.set_known_languages(vec!["Orcish".into(), "Common".into()]);
        s.set_known_languages(vec!["Orcish".into(), "Common".into()]);
        assert_eq!(s.eval::<i64>("return GetNumLaguages()").unwrap(), 2);
        assert_eq!(
            s.eval::<String>("return GetLanguageByIndex(2)").unwrap(),
            "Common"
        );
        assert_eq!(
            s.eval::<i64>("return FIRED").unwrap(),
            1,
            "the list changing fires once; the same list again is silent"
        );
    }
}
