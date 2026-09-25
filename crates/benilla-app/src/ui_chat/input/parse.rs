//! What a submitted slash line means: [`parse_line`] resolves it through the boot-built command
//! table ([`super::super::commands`], the shipped `GlobalStrings.lua` aliases) into a
//! [`ParsedChat`] for [`super`] to execute. A typed line reaches it only through benilla's own
//! `SlashCmdList` entries, once the stock `ChatEdit_ParseText` finds no built-in; a probe's line
//! reaches it directly.

use crate::ui_chat::commands::{Command, DevCmd, SlashCommands, SlashIndex};

/// Escape a typed string for a Lua double-quoted literal: `\` and `"` are escaped, and line
/// terminators are dropped, since one would end the statement.
pub(in crate::ui_chat) fn escape_lua_string(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .flat_map(|c| {
            let escaped = matches!(c, '\\' | '"');
            escaped
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect()
}

/// A social slash line as the reference runs it: `SlashCmdList["<KEY>"](<arg>)`.
fn social_body(key: &str, args: &str) -> ParsedChat {
    ParsedChat::Lua {
        body: format!("SlashCmdList[\"{key}\"](\"{}\")", escape_lua_string(args)),
    }
}

/// What a trimmed slash line resolves to.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::ui_chat) enum ParsedChat {
    /// `/r [text]`: reply to the last tell.
    Reply { text: String },
    /// `/join <name> [password]`.
    Join { name: String, password: String },
    /// `/leave <name>`.
    Leave { name: String },
    /// `/chatlist <name>`: the channel's member roster.
    ChatList { name: String },
    /// `/random [min] [max]`: bare is 1-100, one number is 1-N.
    Random { min: u32, max: u32 },
    /// `/played`: `CMSG_PLAYED_TIME`.
    Played,
    /// `/shot`: the camera pose as a capture `Scenario` line, to chat and `shots.txt`.
    Shot,
    /// `/liquid`: the interior claim, liquid verdict and candidate footprints at the feet.
    Liquid,
    /// `/reaction [name]`: every input and rung of the reaction ladder for one unit.
    Reaction { name: Option<String> },
    /// `/convertraid` (`CMSG_GROUP_RAID_CONVERT`), benilla's own: 1.12 converts only from the
    /// RaidFrame's Convert button.
    ConvertRaid,
    /// `/help`.
    Help,
    /// An `EmotesText` command (`/wave` is 101), sent as `CMSG_TEXT_EMOTE` at the selection.
    TextEmote(u32),
    /// A channel verb the stock handler parsed and the VM queued; never from [`parse_line`].
    Channel(benilla_ui::script::ChannelCommand),
    /// `/castvis`: a locally synthesized [`crate::creature_anim::CastEvent`], at the selection
    /// or self.
    CastVis {
        spell_id: u32,
        kind: crate::creature_anim::CastEventKind,
        /// A pure dest cast (no hits, a point): the only shape that reaches the location fallback.
        ground: bool,
    },
    /// A reference handler that is one Lua call (`InitiateTrade("target")`, `RunScript(msg)`, …).
    Lua { body: String },
    /// `/quit`: `Quit()`, through the game menu Exit button's queue and countdown.
    Quit,
    /// `/logout`: `Logout()`, the game menu's route back to character select.
    Logout,
    /// `/chattest`: one synthetic line of every kind, notice and link form.
    ChatTest,
    /// `/invite` (`CMSG_GROUP_INVITE`): bare falls back to a selected player's name, else does
    /// nothing (`GetSlashCmdTarget`, `ChatFrame.lua:650-658`).
    Invite { name: Option<String> },
    /// `/uninvite` (`CMSG_GROUP_UNINVITE`), with the same fallback.
    Uninvite { name: Option<String> },
    /// `/promote` (`CMSG_GROUP_SET_LEADER`, a guid resolved from the roster), same fallback.
    Promote { name: Option<String> },
    /// `/duel [name]` (`StartDuel`), with the same fallback.
    Duel { name: Option<String> },
    /// `/forfeit`: `CancelDuel()`, one opcode for decline, cancel and forfeit.
    Forfeit,
    /// `/pvp`: `TogglePVP()`.
    Pvp,
    /// `/partytest [lead|raid|invite|mark|ping|off]`: a synthetic party or 25-member raid, a fake
    /// invite, a local skull on the target or a member's minimap ping. While the roster is
    /// synthetic, the popup's group-mutating rows apply locally
    /// ([`crate::ui_party::GroupState::test`]).
    PartyTest { arg: String },
    /// `/target [name]` (`TargetByName`): creatures and players, case-insensitive, whole name or
    /// longest common prefix, with no range, cone, liveness or hostility gate.
    Target { name: Option<String> },
    /// `/assist [name]`: a named basis is a player (typemask `0x10`); bare assists the target.
    Assist { name: Option<String> },
    /// `/follow [name]`: a named subject is a living, assistable player (filter mode 2); bare
    /// follows the selection. Client-side, nothing on the wire.
    Follow { name: Option<String> },
    /// `/macrohelp`: `ChatFrame_DisplayMacroHelpText`'s five `MACRO_HELP_TEXT_LINE`s.
    MacroHelp,
    /// `/reload`, benilla's own: the in-world rebuild `ReloadUI()` queues.
    ReloadUi,
    /// A `/console` line the CVar store did not consume, for the `ConsoleCommand` registry
    /// ([`crate::console::execute`]).
    Console { line: String },
    /// A slash line matching neither a command nor an `EmotesText` name.
    Unknown,
}

/// The name half of `GetSlashCmdTarget` (`ChatFrame.lua:650-665`), shared by the stock commands
/// that take a name. Its gsub is a trim, not a tokenizer, so "Kobold Vermin" is one name; `args`
/// arrives trimmed. Empty defers to the caller's fallback (`target_player_name`). The reference
/// also expands `player`, `target`, `^party[1-4]` and `^raid[0-9]` through `UnitName`; this
/// does not, so a unit token passes through as a literal name.
fn slash_target_name(args: &str) -> Option<String> {
    (!args.is_empty()).then(|| args.to_string())
}

/// Parse a trimmed slash line through the boot-built command table; a line with no slash is
/// [`ParsedChat::Unknown`]. The split is `ChatEdit_ParseText`'s (`ChatFrame.lua:2099-2105`): the
/// command is the first word and the argument the rest, so `/wave Bob` is `/wave` with an argument.
pub(in crate::ui_chat) fn parse_line(table: &SlashCommands, line: &str) -> ParsedChat {
    let Some(rest) = line.strip_prefix('/') else {
        return ParsedChat::Unknown;
    };
    let rest = rest.trim();
    let (cmd, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let args = args.trim();
    match table.lookup(cmd) {
        Some(Command::Slash(index)) => slash_command(index, args),
        // The alias table did the `DoEmote(token)` resolve at boot: `/lol` arrives as LAUGH's id.
        Some(Command::Emote { text_id }) => ParsedChat::TextEmote(text_id),
        Some(Command::Dev(dev)) => dev_command(dev, args),
        // `HELP_TEXT_SIMPLE`'s case, after the drain offers the line to `SlashCmdList`.
        None => ParsedChat::Unknown,
    }
}

/// `s` as an escaped Lua short string literal. A long bracket cannot quote it: a `/console`
/// argument may hold `]]`, and the 1.12 lexer has no bracket levels (the `[` arm at `0x6ff771`
/// compares once, `read_long_string` `0x700010` reads `=` as content), so `[=[` fails in
/// `prefixexp` (`0x6fde40`). `\` and `"` escape themselves, `\n` `\r` `\t` take their names, and
/// any other control byte a three-digit `\ddd`, so a following digit is not swallowed.
pub(in crate::ui_chat) fn lua_quoted_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The per-command argument grammar: each arm is the reference handler's body reduced to what it
/// does with `msg`.
fn slash_command(index: SlashIndex, args: &str) -> ParsedChat {
    use SlashIndex as S;
    match index {
        S::Reply => ParsedChat::Reply {
            text: args.to_string(),
        },
        S::Join => {
            let (name, password) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
            if name.is_empty() {
                // Unknown hands the line to the stock handler, which prints `CHAT_JOIN_HELP`.
                ParsedChat::Unknown
            } else {
                ParsedChat::Join {
                    name: name.to_string(),
                    password: password.trim().to_string(),
                }
            }
        }
        S::Leave => match args.split_whitespace().next() {
            Some(name) => ParsedChat::Leave {
                name: name.to_string(),
            },
            None => ParsedChat::Unknown,
        },
        S::ListChannel => match args.split_whitespace().next() {
            Some(name) => ParsedChat::ChatList {
                name: name.to_string(),
            },
            // Unknown hands a bare line to the stock handler, which runs `ListChannels()`.
            None => ParsedChat::Unknown,
        },
        // The stock bodies: a probe's `/afk` takes the same `SendChatMessage` path, echo, default
        // text and mirror included, as a typed one.
        S::ChatAfk => social_body("CHAT_AFK", args),
        S::ChatDnd => social_body("CHAT_DND", args),
        S::Random => {
            let mut nums = args
                .split_whitespace()
                .filter_map(|w| w.parse::<u32>().ok());
            let (a, b) = (nums.next(), nums.next());
            let (min, max) = match (a, b) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) => (1, a),
                _ => (1, 100),
            };
            ParsedChat::Random { min, max }
        }
        S::Played => ParsedChat::Played,
        S::Help => ParsedChat::Help,
        S::Logout => ParsedChat::Logout,
        S::Quit => ParsedChat::Quit,
        // A bare command falls back to the selected player (`GetSlashCmdTarget`).
        S::Invite => ParsedChat::Invite {
            name: slash_target_name(args),
        },
        S::Uninvite => ParsedChat::Uninvite {
            name: slash_target_name(args),
        },
        S::Promote => ParsedChat::Promote {
            name: slash_target_name(args),
        },
        S::Duel => ParsedChat::Duel {
            name: slash_target_name(args),
        },
        // The whole trimmed argument is the name: `/target Kobold Vermin`.
        S::Target => ParsedChat::Target {
            name: slash_target_name(args),
        },
        S::Assist => ParsedChat::Assist {
            name: slash_target_name(args),
        },
        S::Follow => ParsedChat::Follow {
            name: slash_target_name(args),
        },
        S::DuelCancel => ParsedChat::Forfeit,
        S::Pvp => ParsedChat::Pvp,
        // The stock bodies, with the whole argument: `/who`'s is a filter expression.
        S::Who => social_body("WHO", args),
        S::Friends => social_body("FRIENDS", args),
        S::RemoveFriend => social_body("REMOVEFRIEND", args),
        S::Ignore => social_body("IGNORE", args),
        S::Unignore => social_body("UNIGNORE", args),
        // The reference's one-line bodies, run as Lua.
        S::Trade => ParsedChat::Lua {
            body: "InitiateTrade(\"target\")".into(),
        },
        S::Inspect => ParsedChat::Lua {
            body: "InspectUnit(\"target\")".into(),
        },
        S::LootFfa => ParsedChat::Lua {
            body: "SetLootMethod(\"freeforall\")".into(),
        },
        S::LootRoundRobin => ParsedChat::Lua {
            body: "SetLootMethod(\"roundrobin\")".into(),
        },
        // `LOOT_MASTER` does nothing without a name or player target (`if GetSlashCmdTarget(msg)`).
        S::LootMaster => match slash_target_name(args) {
            Some(name) => ParsedChat::Lua {
                body: format!(
                    "SetLootMethod(\"master\", \"{}\")",
                    escape_lua_string(&name)
                ),
            },
            None => ParsedChat::Unknown,
        },
        // `SlashCmdList["CAST"]` (`ChatFrame.lua:1120`): `CastSpellByName(msg)` unless empty.
        S::Cast => {
            if args.is_empty() {
                ParsedChat::Unknown
            } else {
                ParsedChat::Lua {
                    body: format!("CastSpellByName(\"{}\")", escape_lua_string(args)),
                }
            }
        }
        // `SlashCmdList["MACRO"]` (`ChatFrame.lua:1112`): `ShowMacroFrame()`. Its `RunMacro`
        // branch is commented out, and the 1.12 client registers no `RunMacro`.
        S::MacroUi => ParsedChat::Lua {
            body: "ShowMacroFrame()".into(),
        },
        S::MacroHelp => ParsedChat::MacroHelp,
        // `/reload` is the rebuild `/console reloadUI` runs (`0x4035f0`, which reads no arguments).
        S::ReloadUi => ParsedChat::ReloadUi,
        S::ConvertRaid => ParsedChat::ConvertRaid,
        // `/errors`, benilla's own: toggles the script error log, a FrameXML window;
        // `/errors clear` empties it.
        S::ScriptErrors => ParsedChat::Lua {
            body: if args.trim().eq_ignore_ascii_case("clear") {
                "BenillaScriptLog_Clear()".into()
            } else {
                "BenillaScriptLog_Toggle()".into()
            },
        },
        // `SlashCmdList["CONSOLE"]` (`ChatFrame.lua:671`) is `ConsoleExec(msg)`. A typed line runs
        // it; one that skipped the edit box calls the same verb, so a CVar line writes the CVar
        // and anything else comes back through `engine_verbs`.
        S::Console => ParsedChat::Lua {
            body: format!("ConsoleExec({})", lua_quoted_string(args)),
        },
        // `RunScript(msg)`: the typed text is the chunk, unescaped.
        S::Script => {
            if args.is_empty() {
                ParsedChat::Unknown
            } else {
                ParsedChat::Lua {
                    body: args.to_string(),
                }
            }
        }
    }
}

/// benilla's own instruments' grammar ([`DevCmd`]), in the command table only in a dev build.
fn dev_command(dev: DevCmd, args: &str) -> ParsedChat {
    match dev {
        // `/castvis <spell_id> [go|ground|fail]`: bare starts the precast, `go` releases at the
        // selection, `ground` releases as a pure dest cast ahead of the player, `fail` reaps.
        DevCmd::CastVis => {
            use crate::creature_anim::CastEventKind;
            let mut words = args.split_whitespace();
            let Some(spell_id) = words.next().and_then(|w| w.parse::<u32>().ok()) else {
                return ParsedChat::Unknown;
            };
            let (kind, ground) = match words.next() {
                None => (CastEventKind::Start, false),
                Some(w) if w.eq_ignore_ascii_case("go") => (CastEventKind::Go, false),
                Some(w) if w.eq_ignore_ascii_case("ground") => (CastEventKind::Go, true),
                Some(w) if w.eq_ignore_ascii_case("fail") => (CastEventKind::Fail, false),
                Some(_) => return ParsedChat::Unknown,
            };
            ParsedChat::CastVis {
                spell_id,
                kind,
                ground,
            }
        }
        DevCmd::ChatTest => ParsedChat::ChatTest,
        DevCmd::PartyTest => ParsedChat::PartyTest {
            arg: args.to_ascii_lowercase(),
        },
        DevCmd::Shot => ParsedChat::Shot,
        DevCmd::Liquid => ParsedChat::Liquid,
        DevCmd::Reaction => ParsedChat::Reaction {
            name: slash_target_name(args),
        },
    }
}
