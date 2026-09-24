//! **`SendChatMessage`** — the addon's own line into the chat wire.
//!
//! 24 corpus addons call it, and it is the one verb that makes an addon able to *say* anything:
//! every "announce the pull", every raid warning helper, every whisper-the-invite bot is this one
//! call. It is `function engine` in the captured `_G` — the engine takes the line and the wire
//! sends it, with no FrameXML in between.
//!
//! ```lua
//! SendChatMessage(text [, chatType [, language [, channel/target]]])
//! ```
//!
//! `chatType` defaults to `"SAY"`; `language` is accepted and **ignored** (`0x49f1e0`); the fourth
//! argument is the whisper target for `"WHISPER"` and the channel name or number for
//! `"CHANNEL"`, and is unused otherwise.
//!
//! ## The seam
//!
//! Same shape as [`super::social`]'s: the verb queues a [`ChatSend`] and the app drains it
//! ([`super::UiScript::take_chat_sends`]) into `ClientCommand::Chat`. It deliberately does **not**
//! go through the chat edit box's own drain: that path runs the slash grammar, and an addon's
//! `SendChatMessage("/dance", "SAY")` must *say the four characters*, not dance. The reference has
//! the same split — `ChatEdit_SendText` parses, `SendChatMessage` does not.
//!
//! ## What is not carried, and why
//!
//! **`language`.** The reference resolves a language id to the garble table the receiver's client
//! reverses. benilla sends every line in the speaker's own tongue because the app's chat command
//! carries no language field yet, and inventing one here would put the id in a queue nothing
//! reads. Accepted and dropped, said out loud rather than silently — an addon passing it is not
//! wrong, we are incomplete.
//!
//! **No rate limit and no length cap.** The reference truncates at 255 and the *server* throttles.
//! Ours passes the line through; vmangos enforces both, so a misbehaving addon meets the same wall
//! it would meet on a real server rather than a client-side one we made up.

use mlua::{Lua, MultiValue, Value};

use super::Model;

impl super::UiScript {
    /// Push the player's **default chat language** — the name `GetDefaultLanguage()` answers.
    /// `None` (the default) is the reference's own no-player-object state, which returns **zero
    /// Lua values**, not `nil`. The app resolves it once per world entry from
    /// `ChrRaces.BaseLanguage` × `Languages.dbc` (`benilla_formats::DefaultLanguages`).
    pub fn set_default_language(&mut self, name: Option<String>) {
        self.model_mut().default_language = name;
    }
}

/// One queued outbound chat line (`SendChatMessage`), drained by the app into the wire.
///
/// Plain data — [`super::social::SocialRequest`]'s twin, and deliberately *not* the app's own
/// `ClientCommand::Chat`: this crate has no wire types and must not grow one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSend {
    /// The line, verbatim. Never parsed here — a leading `/` is four characters, not a command.
    pub text: String,
    /// The chat type token, uppercased (`"SAY"`, `"YELL"`, `"PARTY"`, `"GUILD"`, `"WHISPER"`,
    /// `"CHANNEL"`, `"EMOTE"`, …). Uppercased because the reference's own token compare is, and
    /// addons write `"say"` as often as `"SAY"`.
    pub chat_type: String,
    /// The whisper target (`"WHISPER"`) or the channel name/number (`"CHANNEL"`); `None`
    /// otherwise. Kept as a string for the channel case, where an addon passes either `"General"`
    /// or `1` and the app resolves both.
    pub target: Option<String>,
}

impl super::UiScript {
    /// The languages this character **knows**, in `Languages.dbc` row order — what
    /// `GetNumLaguages`/`GetLanguageByIndex` walk. The app folds it the reference's way:
    /// `0x4b25b0` stores `[languageId] = spellId` for every known spell whose `Effect_1 == 39`,
    /// and `0x5ec720` answers non-zero only when that spell's skill line is in the player's
    /// skill block. Fires `LANGUAGE_LIST_CHANGED`
    /// (`0x49b970`, event 0x102) when the list moves.
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
    // GetNumLaguages() — `0x49fb30`, the binary's own spelling. Walks `Languages.dbc` and counts
    // the rows `0x5ec720` answers non-zero for; **one number**.
    lua.globals().set(
        "GetNumLaguages",
        lua.create_function(|lua, _ignored: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.known_languages.len() as i64)
        })?,
    )?;
    // GetLanguageByIndex(i) — `0x49fbe0`: the same walk, pushing **one string** — the
    // `Name_lang` of the i-th (1-based) *known* language. Positional among the known rows,
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
    // GetDefaultLanguage() → **exactly ONE value, a string** — or **zero values**, which is not
    // the same thing (`0x49fcd0`).
    //
    // The binding takes **no arguments** (no arg-presence check and no arg-fetch call anywhere in
    // its 94 bytes — contrast its sibling `GetLanguageByIndex 0x49fbe0`, which opens with
    // `0x6f34d0`), so the corpus's `GetDefaultLanguage("player")` — `Auctioneer/AucAskPrice.lua:42`
    // and `Enchantrix/EnchantrixBarker.lua:149` both write it — is harmlessly ignored, here as
    // there.
    //
    // There is exactly one push in the body and it is `0x6f3890` (push-STRING): the numeric
    // language id is consumed as a table index and never reaches Lua, so this is not `(name, id)`
    // and not an id. All **four** failure edges — no player object, a negative id, an id past the
    // language count, a null record — converge on `0x49fd2a xor eax,eax; ret`, i.e. **zero Lua
    // values**. That is shape 2 of the argument ABI ([`super::binding_abi`]) and it is the one
    // place in this repo where the distinction is observable: the call's return-list COUNT is `0`
    // outside the world and `1` inside it, while a single-value caller reads `nil` either way.
    // Returning `nil` here would be a quiet divergence, so we do not.
    //
    // **Its sibling is misspelled in the binary, and is deliberately NOT registered here.** The
    // `.data` `{const char* name, void* fn}` record at `0x843628` names `0x49fb30`
    // **`GetNumLaguages`** — and a whole-image byte search for the correct spelling returns **zero
    // hits** (with `GetNumLaguages` and `GetDefaultLanguage` as satisfied positive controls), so
    // the typo is the only Lua-visible name and a faithful client would ship it. What is missing
    // is not the name but the *answer*: `0x49fb30`'s body was not carved beyond "returns through
    // the push-number helper", no corpus addon calls it at all, and registering it would mean
    // inventing a count. The name is recorded here so that whoever carves the body knows how to
    // spell the global.
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
                // The reference's default is SAY — an addon that passes only a string is saying it.
                let chat_type = chat_type
                    .unwrap_or_else(|| "SAY".into())
                    .to_ascii_uppercase();
                // The fourth argument is a name for WHISPER and a name-or-number for CHANNEL, so
                // it arrives as either a string or a number and is normalised to text here rather
                // than at every consumer.
                let target = match target {
                    Value::String(s) => Some(s.to_string_lossy()),
                    Value::Integer(n) => Some(n.to_string()),
                    Value::Number(n) => Some(format!("{n:.0}")),
                    _ => None,
                };
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

    /// The default is SAY, the type is uppercased, and **a leading slash is text**.
    ///
    /// That last one is the whole reason this verb has its own queue rather than reusing the chat
    /// box's: the box's drain runs the slash grammar, and an addon announcing "/dance" wants the
    /// six characters said, not the emote played. The reference splits the same way.
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
        // Drained, not re-read.
        assert!(s.take_chat_sends().is_empty());
    }

    /// The fourth argument is a whisper target or a channel, and a channel arrives as either a
    /// name or a number — both normalise to text so one consumer handles both.
    #[test]
    fn the_target_argument_takes_a_name_or_a_channel_number() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendChatMessage("hi", "WHISPER", nil, "Bob")"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, 1)"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, "General")"#)
            .unwrap();
        let sent = s.take_chat_sends();
        assert_eq!(sent[0].target.as_deref(), Some("Bob"));
        assert_eq!(sent[1].target.as_deref(), Some("1"));
        assert_eq!(sent[2].target.as_deref(), Some("General"));
        assert_eq!(sent[1].chat_type, "CHANNEL");
    }

    /// `language` is accepted and dropped — an addon passing it must not error, and the module
    /// doc says why nothing reads it.
    #[test]
    fn the_language_argument_is_accepted_and_ignored() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendChatMessage("hi", "SAY", 7)"#).unwrap();
        assert_eq!(s.take_chat_sends().len(), 1);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// **One string, or ZERO values — never `nil`.** The four failure edges reach
    /// `0x49fd2a xor eax,eax; ret` *without* passing through `luaL_error`, which is the only place
    /// the "returns nothing" shape is real. A single-value caller cannot tell the two apart; the
    /// return-list count can ([`UiScript::arity`]), and both corpus callers feed the result straight to
    /// `SendChatMessage`, where the difference is an argument that exists versus one that does not.
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
        // The binding reads NO arguments; the corpus passes `"player"` anyway and must not error.
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
