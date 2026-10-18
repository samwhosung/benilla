//! `SendChatMessage(text [, chatType [, language [, target]]])` (`0x49f1e0`): the engine verb an
//! addon speaks through, with no FrameXML between it and the wire. `chatType` defaults to
//! `"SAY"`, also when it is not a string or a number; the fourth argument is the whisper target or
//! the channel's number.
//!
//! The call returns quietly with no local player (`0x49f223`) and raises the usage error for a text
//! that is no string (`0x49f77c`). It raises where the reference does, ahead of anything queued: a
//! string `chatType` outside [`SEND_CHAT_TOKENS`] raises `Unknown chat type` (`0x49f27f`), and past the empty-line gate and
//! the language, a WHISPER with no target raises `Whisper message missing target player!`
//! (`0x49f325`) and a CHANNEL with none `Channel send missing channel number` (`0x49f535`). A
//! CHANNEL target naming no joined channel is no error (`0x49f4ea`): the drain ends the call.
//!
//! It queues a [`ChatSend`] rather than going through the edit box, whose drain runs the slash
//! grammar: an addon's `SendChatMessage("/dance")` says the characters. The reference splits the
//! same way, `ChatEdit_SendText` parsing and `SendChatMessage` not.
//!
//! `language` starts as the speaker's race base language (`0x5ec890`, at `0x49f2aa`). A string or
//! number third argument is matched by name, case-insensitively, against every `Languages.dbc`
//! row (`0x49f8a0`), known to the character or not, and a miss raises `Unknown language`
//! (`0x844afc`, at `0x49f2e1`): a number is its text, so `7` raises. Refusing a tongue the
//! character never learned is the server's (vmangos `ChatHandler.cpp:175`).
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

/// The chat types a string `chatType` names (the table `0x49f7a0` builds, tested at `0x49f276`),
/// matched ignoring case (`SStrCmpI` `0x64a4c0`). `RAID_LEADER` and `BATTLEGROUND_LEADER` are
/// not in it: the reference derives those types itself from `RAID` and `BATTLEGROUND`
/// (`0x49f3bf`, `0x49f48b`).
pub const SEND_CHAT_TOKENS: [&str; 13] = [
    "SAY",
    "PARTY",
    "RAID",
    "GUILD",
    "OFFICER",
    "YELL",
    "WHISPER",
    "EMOTE",
    "CHANNEL",
    "AFK",
    "DND",
    "RAID_WARNING",
    "BATTLEGROUND",
];

/// `Usage: SendChatMessage(text [,type] [,language] [,targetPlayer])`, `0x844a08`.
const USAGE_SEND_CHAT_MESSAGE: &str =
    "Usage: SendChatMessage(text [,type] [,language] [,targetPlayer])";

/// `Unknown chat type`, `0x844b10`.
const UNKNOWN_CHAT_TYPE: &str = "Unknown chat type";

/// `Whisper message missing target player!`, `0x844ad4`.
const WHISPER_MISSING_TARGET: &str = "Whisper message missing target player!";

/// `Channel send missing channel number`, `0x844a7c`.
const CHANNEL_MISSING_NUMBER: &str = "Channel send missing channel number";

/// One queued `SendChatMessage` line, plain data: this crate holds no wire types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSend {
    /// The line, verbatim: a leading `/` is text, not a command.
    pub text: String,
    /// The chat type token (`"SAY"`, `"WHISPER"`, `"CHANNEL"`, …), uppercased, since the
    /// reference's compare ignores case; always one of [`SEND_CHAT_TOKENS`].
    pub chat_type: String,
    /// The fourth argument as `lua_tostring` gives it (`0x49f306`): the whisper target, or the
    /// channel number the app resolves to a joined channel's name. For WHISPER and CHANNEL it is
    /// never absent or empty past the empty-line gate, which raise without one; an empty line is
    /// queued unchecked and the drain drops it.
    pub target: Option<String>,
    /// The `Languages.dbc` id the third argument named; `None` is the speaker's default.
    pub language: Option<u32>,
}

impl ChatSend {
    /// The empty-line gate (`0x49f28d`-`0x49f2a1`): a line whose first byte is NUL ends the call,
    /// before the language is read or anything is sent, unless its type is AFK (`0x14`) or DND
    /// (`0x15`). The chat-type lookup and its "Unknown chat type" (`0x49f27f`) come first, in the
    /// binding.
    pub fn ends_at_empty_line(&self) -> bool {
        ends_at_empty_line(&self.text, &self.chat_type)
    }
}

/// [`ChatSend::ends_at_empty_line`] on the binding's arguments, before the line is built.
fn ends_at_empty_line(text: &str, chat_type: &str) -> bool {
    text.bytes().next().unwrap_or(0) == 0 && !matches!(chat_type, "AFK" | "DND")
}

/// What `SendChatMessage` reads of the group before it sends: the party, raid and leader tests of
/// `0x49f343`-`0x49f4b2`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChatGroup {
    /// The first party GUID `[0xbc6f48]` is non-zero (`0x49f343`).
    pub in_party: bool,
    /// The raid member count `[0xb713e0]` (`0x49f379`).
    pub raid_members: usize,
    /// The player's GUID is the group leader's `[0xbc75f8]` (`0x49f3a4`, `0x49f463`): a party
    /// leader leads too, and a zero GUID matches nobody.
    pub leads: bool,
    /// `IsRaidLeaderOrAssistant` (`0x4bb080`) on the player: a raid member of rank above 0, or the
    /// leader.
    pub leads_or_assists: bool,
}

impl super::UiScript {
    /// The group facts `SendChatMessage`'s gates and leader promotion read.
    pub fn chat_group(&self) -> ChatGroup {
        let model = self.model_ref();
        let party = &model.party;
        let own = party.own_guid;
        let leads = own != 0 && own == party.leader_guid;
        // `0x4bb080`: a zero GUID or one off the roster fails; a member above rank 0 passes, and
        // one at rank 0 passes only as the group leader.
        let leads_or_assists = party
            .raid
            .iter()
            .find(|m| own != 0 && m.guid == own)
            .is_some_and(|m| m.rank > 0 || leads);
        ChatGroup {
            in_party: party.members.first().is_some_and(|m| m.guid != 0),
            raid_members: party.raid.len(),
            leads,
            leads_or_assists,
        }
    }
}

impl super::UiScript {
    /// Push `Languages.dbc` as `(ID, Name_lang)` rows in file order, the table `SendChatMessage`
    /// matches its language name against (`[0xc0db40]`, count `[0xc0db44]`).
    pub fn set_language_table(&mut self, rows: Vec<(u32, String)>) {
        self.model_mut().language_table = rows;
    }
}

/// `0x49f8a0`: the first row, in file order, whose name equals `name` ignoring case (`SStrCmpI`
/// `0x64a4c0`, unbounded length), checked against every row, not only the known languages.
fn language_id(table: &[(u32, String)], name: &str) -> Option<u32> {
    table
        .iter()
        .find(|(_, row)| row.eq_ignore_ascii_case(name))
        .map(|&(id, _)| id)
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetLanguageByIndex(index)` (`0x844b9c`, `0x49fbfc`).
            let index = crate::script::binding_abi::index_arg(
                lua,
                index,
                "Usage: GetLanguageByIndex(index)",
            )?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let name = index
                .checked_sub(1)
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
            |lua, (text, chat_type, language, target): (Value, Value, Value, Value)| {
                // No local player ends the call quietly, ahead of every check (`0x49f219`-
                // `0x49f223` to `0x49f78a`).
                if !super::battlefield_queue::player_exists(
                    &lua.app_data_ref::<Model>().expect("model app_data"),
                ) {
                    return Ok(());
                }
                // `lua_isstring(1)` (`0x49f230`): a string or a number, else the usage error
                // (`0x844a08`, at `0x49f77c`).
                let text = super::binding_abi::string_arg(lua, text, USAGE_SEND_CHAT_MESSAGE)?;
                // `lua_isstring(2)` (`0x49f25c`): anything that is not a string or a number leaves
                // the type at 0, SAY (`0x49f255`).
                let chat_type = super::binding_abi::optional_string(lua, &chat_type)
                    .unwrap_or_else(|| "SAY".into())
                    .to_ascii_uppercase();
                // The type check precedes the empty-line gate (`0x49f276`-`0x49f285`).
                if !SEND_CHAT_TOKENS.contains(&chat_type.as_str()) {
                    return Err(mlua::Error::runtime(UNKNOWN_CHAT_TYPE));
                }
                // The language is read only past the empty-line gate. The line is queued
                // anyway and the drain applies the gate.
                let reads_language = !ends_at_empty_line(&text, &chat_type);
                let language = match super::binding_abi::optional_string(lua, &language) {
                    Some(name) if reads_language => {
                        let model = lua.app_data_ref::<Model>().expect("model app_data");
                        let id = language_id(&model.language_table, &name);
                        Some(
                            id.ok_or_else(|| mlua::Error::RuntimeError("Unknown language".into()))?,
                        )
                    }
                    _ => None,
                };
                // `lua_isstring` then `lua_tostring` (`0x49f2f6`, `0x49f306`): a number is its
                // text, so `2.7` reaches the channel's `SStrToInt` as "2.7", which reads 2.
                let target = super::binding_abi::optional_string(lua, &target);
                // Past the gate and the language, a WHISPER or CHANNEL with a NULL or empty
                // target raises (`0x49f310`-`0x49f32b`, `0x49f4ca`-`0x49f4d6`, `0x49f535`).
                // NULL, or a first byte of NUL (`cmp byte [esi],0`), as the empty-line gate reads.
                if reads_language
                    && target
                        .as_deref()
                        .is_none_or(|t| t.bytes().next().unwrap_or(0) == 0)
                {
                    match chat_type.as_str() {
                        "WHISPER" => return Err(mlua::Error::runtime(WHISPER_MISSING_TARGET)),
                        "CHANNEL" => return Err(mlua::Error::runtime(CHANNEL_MISSING_NUMBER)),
                        _ => {}
                    }
                }
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .chat_sends
                    .push(ChatSend {
                        text,
                        chat_type,
                        target,
                        language,
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

    /// A VM with a local player, as every in-world `SendChatMessage` has (`0x49f223`).
    fn vm() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(crate::script::UnitState {
                exists: true,
                name: Some("Probe".into()),
                ..Default::default()
            }),
        );
        s
    }

    #[test]
    fn send_chat_message_queues_a_line_without_parsing_it() {
        let mut s = vm();
        s.run(r#"SendChatMessage("hello")"#).unwrap();
        s.run(r#"SendChatMessage("/dance", "say")"#).unwrap();
        assert_eq!(
            s.take_chat_sends(),
            vec![
                ChatSend {
                    text: "hello".into(),
                    chat_type: "SAY".into(),
                    target: None,
                    language: None,
                },
                ChatSend {
                    text: "/dance".into(),
                    chat_type: "SAY".into(),
                    target: None,
                    language: None,
                },
            ]
        );
        assert!(s.take_chat_sends().is_empty());
    }

    #[test]
    fn the_target_argument_is_its_lua_tostring_text() {
        let mut s = vm();
        s.run(r#"SendChatMessage("hi", "WHISPER", nil, "Bob")"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, 1)"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, "General")"#)
            .unwrap();
        s.run(r#"SendChatMessage("lf1m", "CHANNEL", nil, 2.7)"#)
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
        assert_eq!(sent[1].chat_type, "CHANNEL");
    }

    /// What `pcall` leaves of `SendChatMessage(args)`: `Ok(())` or the error text.
    fn call(s: &UiScript, args: &str) -> Result<(), String> {
        let (ok, err): (bool, String) = s
            .eval(&format!(
                "local ok, err = pcall(function() SendChatMessage({args}) end) \
                 return ok, tostring(err)"
            ))
            .unwrap();
        if ok {
            Ok(())
        } else {
            Err(err)
        }
    }

    /// `0x49f276`-`0x49f285`: a string outside the table at `0x49f7a0` raises in the call.
    #[test]
    fn an_unknown_chat_type_raises_and_queues_nothing() {
        let mut s = vm();
        with_languages(&mut s);
        for ty in [
            r#""INSTANCE_CHAT""#,
            r#""BOGUS""#,
            r#""""#,
            r#""SAY ""#,
            r#""RAID_LEADER""#,
            r#""BATTLEGROUND_LEADER""#,
            "7",
        ] {
            let err = call(&s, &format!(r#""hi", {ty}"#)).expect_err(ty);
            assert!(err.contains("Unknown chat type"), "{ty}: {err}");
        }
        // Ahead of the empty-line gate (`0x49f28d`) and the language (`0x49f2c5`).
        let err = call(&s, r#""", "BOGUS""#).expect_err("an empty line still checks the type");
        assert!(err.contains("Unknown chat type"), "{err}");
        let err = call(&s, r#""hi", "BOGUS", "Klingon""#).expect_err("type before language");
        assert!(err.contains("Unknown chat type"), "{err}");
        assert!(s.take_chat_sends().is_empty());
        // The compare ignores case (`SStrCmpI`), and every table entry passes.
        for token in SEND_CHAT_TOKENS {
            let target = if matches!(token, "WHISPER" | "CHANNEL") {
                r#", nil, "1""#
            } else {
                ""
            };
            let args = format!(r#""hi", "{}"{target}"#, token.to_lowercase());
            call(&s, &args).expect(token);
        }
        assert_eq!(s.take_chat_sends().len(), SEND_CHAT_TOKENS.len());
    }

    /// `lua_isstring(2)` (`0x49f25c`) refuses anything but a string or a number, which leaves the
    /// type at SAY (`0x49f255`): no error, no conversion failure.
    #[test]
    fn a_chat_type_that_is_no_string_is_say() {
        let mut s = vm();
        for ty in ["nil", "true", "false", "{}", "print"] {
            call(&s, &format!(r#""hi", {ty}"#)).unwrap_or_else(|e| panic!("{ty}: {e}"));
        }
        let sent = s.take_chat_sends();
        assert_eq!(sent.len(), 5);
        assert!(sent.iter().all(|c| c.chat_type == "SAY"), "{sent:?}");
    }

    /// `0x4bb080` and the leader compares (`0x49f3a4`, `0x49f463`).
    #[test]
    fn the_group_facts_follow_the_references_tests() {
        use crate::script::{PartyMemberInfo, PartyState, RaidMemberInfo};
        let member = |guid, rank| RaidMemberInfo {
            guid,
            rank,
            ..Default::default()
        };
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.chat_group(), ChatGroup::default(), "ungrouped");
        let state = |own, leader, rank| PartyState {
            members: vec![PartyMemberInfo {
                name: "B".into(),
                guid: 2,
            }],
            own_guid: own,
            leader_guid: leader,
            raid: vec![member(own, rank), member(2, 0)],
            ..Default::default()
        };
        s.set_party(state(1, 9, 0));
        let g = s.chat_group();
        assert!(g.in_party && g.raid_members == 2);
        assert!(!g.leads && !g.leads_or_assists, "a plain member");
        s.set_party(state(1, 9, 1));
        let g = s.chat_group();
        assert!(!g.leads && g.leads_or_assists, "an assistant");
        s.set_party(state(1, 1, 0));
        let g = s.chat_group();
        assert!(g.leads && g.leads_or_assists, "the leader at rank 0");
        let mut off_roster = state(1, 1, 0);
        off_roster.raid = vec![member(2, 2)];
        s.set_party(off_roster);
        let g = s.chat_group();
        assert!(
            g.leads && !g.leads_or_assists,
            "`0x4bb080` fails off the roster"
        );
        s.set_party(state(0, 0, 0));
        let g = s.chat_group();
        assert!(
            !g.leads && !g.leads_or_assists,
            "a zero guid matches nobody"
        );
    }

    /// `0x49f219`-`0x49f223`: with no local player the call ends quietly, ahead of the usage,
    /// type and language checks; with one, a text that is no string raises the usage error.
    #[test]
    fn no_player_ends_quietly_and_a_non_string_text_raises_usage() {
        let mut bare = UiScript::new().unwrap();
        call(&bare, "nil").expect("no player: no usage error either");
        call(&bare, r#""hi", "BOGUS", "Klingon""#).expect("no player: nothing is checked");
        assert!(bare.take_chat_sends().is_empty());

        let mut s = vm();
        for text in ["nil", "true", "{}", ""] {
            let err = call(&s, text).expect_err(text);
            assert!(
                err.contains("Usage: SendChatMessage(text [,type] [,language] [,targetPlayer])"),
                "{text:?}: {err}"
            );
        }
        call(&s, "42").expect("a number is a text");
        assert_eq!(s.take_chat_sends()[0].text, "42");
        s.set_unit("player", None);
        call(&s, "nil").expect("the player gone: quiet again");
    }

    /// `cmp byte [esi],0` (`0x49f31c`, `0x49f4d3`): a target whose first byte is NUL is empty.
    #[test]
    fn a_target_that_starts_with_nul_is_empty() {
        let mut s = vm();
        let err = call(&s, r#""hi", "WHISPER", nil, "\0Bob""#).expect_err("whisper");
        assert!(
            err.contains("Whisper message missing target player!"),
            "{err}"
        );
        let err = call(&s, r#""hi", "CHANNEL", nil, "\0001""#).expect_err("channel");
        assert!(err.contains("Channel send missing channel number"), "{err}");
        call(&s, r#""hi", "WHISPER", nil, "B\0ob""#).expect("a later NUL ends nothing");
        assert_eq!(s.take_chat_sends().len(), 1);
    }

    /// `0x49f310`-`0x49f32b` and `0x49f4ca`-`0x49f4d6`, `0x49f535`: a WHISPER or CHANNEL whose
    /// fourth argument is not a non-empty string raises, past the empty-line gate and the language.
    #[test]
    fn a_whisper_or_channel_without_a_target_raises_and_queues_nothing() {
        let mut s = vm();
        with_languages(&mut s);
        let whisper = "Whisper message missing target player!";
        let channel = "Channel send missing channel number";
        for target in ["", ", nil, nil", r#", nil, """#, ", nil, {}", ", nil, true"] {
            let err = call(&s, &format!(r#""hi", "WHISPER"{target}"#)).expect_err(target);
            assert!(err.contains(whisper), "{target:?}: {err}");
            let err = call(&s, &format!(r#""hi", "channel"{target}"#)).expect_err(target);
            assert!(err.contains(channel), "{target:?}: {err}");
        }
        assert!(s.take_chat_sends().is_empty());
        // The empty-line gate (`0x49f28d`) and the language (`0x49f2e1`) come first.
        call(&s, r#""", "WHISPER""#).expect("an empty line ends quietly");
        call(&s, r#""", "CHANNEL""#).expect("an empty line ends quietly");
        let err = call(&s, r#""hi", "WHISPER", "Klingon""#).expect_err("language first");
        assert!(err.contains("Unknown language"), "{err}");
        s.take_chat_sends();
        // No other type needs a target, and a target naming no channel is the drain's quiet end
        // (`0x49f4ea`), not an error here.
        call(&s, r#""hi", "SAY""#).unwrap();
        call(&s, r#""hi", "GUILD""#).unwrap();
        call(&s, r#""hi", "CHANNEL", nil, "General""#).unwrap();
        call(&s, r#""hi", "WHISPER", nil, "Bob""#).unwrap();
        let sent = s.take_chat_sends();
        assert_eq!(
            sent.iter()
                .map(|c| c.chat_type.as_str())
                .collect::<Vec<_>>(),
            ["SAY", "GUILD", "CHANNEL", "WHISPER"]
        );
    }

    /// Three `Languages.dbc` rows, in the shipped file's order.
    fn with_languages(s: &mut UiScript) {
        s.set_language_table(vec![
            (1, "Orcish".into()),
            (2, "Darnassian".into()),
            (7, "Common".into()),
        ]);
    }

    #[test]
    fn a_language_name_sends_its_languages_dbc_id() {
        let mut s = vm();
        with_languages(&mut s);
        s.run(r#"SendChatMessage("hi", "SAY", "Darnassian")"#)
            .unwrap();
        s.run(r#"SendChatMessage("hi", "YELL", "dARNASSIAN")"#)
            .unwrap();
        s.run(r#"SendChatMessage("hi", "WHISPER", "orcish", "Bob")"#)
            .unwrap();
        let sent = s.take_chat_sends();
        assert_eq!(sent[0].language, Some(2), "Darnassian's row ID");
        assert_eq!(sent[1].language, Some(2), "SStrCmpI ignores case");
        assert_eq!(
            sent[2].language,
            Some(1),
            "a language the character may not know is sent: the server refuses it"
        );
        assert_eq!(sent[2].target.as_deref(), Some("Bob"));
    }

    #[test]
    fn no_language_argument_speaks_the_default() {
        let mut s = vm();
        with_languages(&mut s);
        s.run(r#"SendChatMessage("hi")"#).unwrap();
        s.run(r#"SendChatMessage("hi", "SAY", nil)"#).unwrap();
        s.run(r#"SendChatMessage("hi", "SAY", {})"#).unwrap();
        s.run(r#"SendChatMessage("hi", "SAY", true)"#).unwrap();
        let sent = s.take_chat_sends();
        assert_eq!(sent.len(), 4);
        assert!(
            sent.iter().all(|c| c.language.is_none()),
            "not a string, so `lua_isstring` keeps the default: {sent:?}"
        );
    }

    #[test]
    fn an_unknown_language_raises_and_sends_nothing() {
        let mut s = vm();
        with_languages(&mut s);
        for arg in [r#""Klingon""#, r#""""#, "7", r#""Darnassian ""#] {
            let err = s
                .run(&format!(r#"SendChatMessage("hi", "SAY", {arg})"#))
                .expect_err(&format!("{arg} names no row"));
            assert!(err.to_string().contains("Unknown language"), "{arg}: {err}");
        }
        assert!(s.take_chat_sends().is_empty());
        // AFK and DND pass the empty-line gate, so their language is read.
        assert!(s.run(r#"SendChatMessage("", "AFK", "Klingon")"#).is_err());
        // An empty line of any other type ends before the language is read.
        s.run(r#"SendChatMessage("", "SAY", "Klingon")"#).unwrap();
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
