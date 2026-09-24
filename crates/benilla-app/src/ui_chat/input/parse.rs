//! What a submitted line MEANS — the grammar half of the chat's outbound side (decision 0288 P5,
//! re-based on the command table in decision 0881). Three resolvers, in the reference's own
//! `ChatEdit_ParseText` order: the chat-TYPE switch ([`parse_enter_type_switch`] — `/say`, `/w`,
//! a channel number), then the ACTION table ([`parse_line`] → [`ParsedChat`], every
//! `SlashCmdList` command benilla registers plus the `/wave`-style emotes), then nothing, which
//! is `HELP_TEXT_SIMPLE`. Doing anything about the result is [`super`]'s (the drain).
//!
//! No aliases live here: which strings reach which arm is [`super::super::commands`]'s table,
//! built from the shipped `GlobalStrings.lua`.

use crate::ui_chat::commands::{Command, DevCmd, SlashCommands, SlashIndex};

/// Escape a player-typed string for embedding in a Lua double-quoted literal: backslashes and
/// quotes are escaped, and the two line terminators are dropped rather than escaped (nothing a
/// slash command means can contain them, and a stray one would end the statement).
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

/// A social slash line as the reference runs it: `SlashCmdList["<KEY>"](<arg>)` (1959).
fn social_body(key: &str, args: &str) -> ParsedChat {
    ParsedChat::Lua {
        body: format!("SlashCmdList[\"{key}\"](\"{}\")", escape_lua_string(args)),
    }
}

/// What a submitted, already-trimmed chat line resolves to (see [`parse_line`]). Kept separate from
/// [`ClientCommand`] so the parser is unit-testable without a live [`crate::sound::EmoteSounds`]
/// resource or a network channel.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::ui_chat) enum ParsedChat {
    /// `/r [text]` — reply to the last received tell (the tell ring is the stock `ChatEdit_*`
    /// Lua's since 1948 — see [`crate::ui_chat::edit`]).
    Reply { text: String },
    /// `/join <name> [password]` (aliases /channel /chan — SLASH_JOIN).
    Join { name: String, password: String },
    /// `/leave <name>` (aliases /chatleave /chatexit).
    Leave { name: String },
    /// `/chatlist <name>` (aliases /chatwho /chatinfo) — the member roster ask.
    ChatList { name: String },
    /// `/random [min] [max]` (aliases /rand /rnd /roll): bare = 1-100, one number = 1-N.
    Random { min: u32, max: u32 },
    /// `/played` — CMSG_PLAYED_TIME.
    Played,
    /// `/shot` — the director's framing instrument (decision 0600): dump the CURRENT world-camera
    /// pose as a ready-to-paste capture `Scenario` (raw WoW eye/look + the rendered game minute)
    /// into chat and `benilla-config/shots.txt`. Client-local, no wire traffic — how a chosen spot
    /// travels from the director's eye to the golden set as exact numbers.
    Shot,
    /// `/liquid` — the swim diagnostic (decision 0634 follow-up): dump the interior claim, the
    /// resolved liquid verdict, and every candidate footprint over the player's feet. Client-local,
    /// no wire traffic. Built when the canal-tunnel "swim in air" survived the first fix — the
    /// question "which surface is claiming this spot, and from which file" had no instrument.
    Liquid,
    /// `/reaction [name]` — the attackability diagnostic: every input the reaction ladder judges
    /// the subject on, the rung that decided, and the resulting `can_attack` verdict. Bare uses
    /// the current target; a name resolves a streamed player (what a scripted probe can reach).
    Reaction { name: Option<String> },
    /// `/convertraid` — convert the party to a raid (`CMSG_GROUP_RAID_CONVERT`, leader only,
    /// server-judged). A benilla addition: 1.12's only trigger is the RaidFrame tab's Convert
    /// button, not built yet — see the [`SlashIndex::ConvertRaid`] doc.
    ConvertRaid,
    /// `/help` (aliases /h /?) — the command summary.
    Help,
    /// A `/name` line that resolved in `EmotesText` (`/wave` → text id 101) — sent as
    /// `CMSG_TEXT_EMOTE` targeted at the current selection.
    TextEmote(u32),
    /// A channel verb the stock `ChatFrame.lua` handler already parsed and the VM queued
    /// (`JoinChannelByName`, `ChannelKick`, … — `benilla_ui::script::ChannelCommand`). Never
    /// produced by [`parse_line`]; it enters the executor from the engine's queue.
    Channel(benilla_ui::script::ChannelCommand),
    /// The `/castvis` **dev instrument** (decision 0099 phase 2): synthesize a cast edge locally
    /// — the exact [`crate::creature_anim::CastEvent`] the wire would produce — on the selection
    /// (else self), no server round-trip. Runtime-available like every current instrument
    /// (`ui_pass::demo_enabled`'s note: decision 0026's compile-time `dev` feature is the recorded
    /// target seam, not yet built); it moves behind that feature when 0026 phase 1 lands.
    CastVis {
        spell_id: u32,
        kind: crate::creature_anim::CastEventKind,
        /// `ground` — send the GO as a **pure dest cast**: empty hit and miss lists, a point on
        /// the wire. That is the only shape that reaches the location fallback (one projectile at
        /// the point, the ground arrival), so without it the instrument cannot see Flare or a
        /// bomb thrown at empty dirt at all. Ignored on the non-GO edges.
        ground: bool,
    },
    /// A command whose reference handler is a **one-line call** into FrameXML — `/trade`
    /// (`InitiateTrade("target")`), `/inspect` (`InspectUnit("target")`), the loot-method trio
    /// (`SetLootMethod(…)`), and `/script`/`/run` (the ref's `RunScript(msg)`, where the typed text
    /// IS the chunk). Running the reference's own call is the 0668 posture: the panel-opening,
    /// permission and error behaviour stays in the FrameXML that owns it instead of being
    /// re-derived in Rust.
    Lua { body: String },
    /// `/quit` `/exit` — the reference's `Quit()`: leave the game entirely, through the same
    /// session queue (and the same confirmation/countdown) the game menu's Exit button uses.
    Quit,
    /// `/logout` — leave the world back to character select (`CMSG_LOGOUT_REQUEST`, decision 0193);
    /// the vanilla client's own command. The confirmed round-trip (`SMSG_LOGOUT_COMPLETE`) flips
    /// [`crate::char_select::ClientState`] back to the roster screen.
    Logout,
    /// `/chattest` — the 0288 instrument: pump one synthetic line of every kind/notice/link form
    /// through the REAL router+composer, so the whole surface is eyeballable in one screen with
    /// no server choreography. Dev-runtime like `/castvis` (decision 0026's compile-time gate is
    /// the recorded target seam).
    ChatTest,
    /// `/invite` `/inv` `/i` — party invite by name (`CMSG_GROUP_INVITE`). `None` = bare, which
    /// falls back to the selected PLAYER's name (the ref's `GetSlashCmdTarget` law,
    /// ChatFrame.lua:650-658 — no name and no player target is a silent no-op).
    Invite { name: Option<String> },
    /// `/uninvite` `/un` `/u` `/kick` — kick by name (`CMSG_GROUP_UNINVITE`), same
    /// name-or-target law.
    Uninvite { name: Option<String> },
    /// `/promote` `/pr` — hand leadership over (`CMSG_GROUP_SET_LEADER`), same name-or-target
    /// law; the 1.12 wire takes a guid, resolved against the roster at dispatch.
    Promote { name: Option<String> },
    /// `/duel [name]` — challenge a player (the duel spell cast at them). `None` = bare, which
    /// falls back to the selected PLAYER exactly like the party commands do — the ref's
    /// `SlashCmdList["DUEL"]` is `if GetSlashCmdTarget(msg) then StartDuel(...)`, so no name and
    /// no player target is a silent no-op.
    Duel { name: Option<String> },
    /// `/forfeit` `/concede` `/yield` — `CancelDuel()`. Takes no argument and needs no state
    /// check: one opcode covers decline/cancel/forfeit and the server reads the intent.
    Forfeit,
    /// `/pvp` — `TogglePVP()` (decision 0646 §3). Takes no argument: the binding has no state
    /// form, and the server reads the toggle from our current preference.
    Pvp,
    /// `/partytest [lead|raid|invite|mark|ping|off]` — the party-frame dev instrument (decision 0434, the
    /// `/chattest` pattern): a synthetic roster through the real apply path (`lead` = the same
    /// roster with US leading, for the leader-only popup rows), a fake pending invite for the
    /// popup, a local skull on the current target (the mark renders without a server echo), or
    /// clear. While the roster is synthetic the drain runs in SANDBOX mode
    /// ([`crate::ui_party::GroupState::test`]): the popup's group-mutating rows — promote,
    /// kick, leave, loot settings, raid marks — apply to the local mirror, so the whole menu
    /// surface is exercisable serverless. `raid` is the same idea one level out (decision 1549):
    /// a synthetic 25-member RAID with us leading, which is the only way the Raid tab's 8×5 grid
    /// is eyeballable without forty accounts — the drag, Ready Check and the kick all apply to
    /// the local mirror there too. `ping` (decision 1596) seats a synthetic *group member's*
    /// minimap ping 35 yd north-east — the one leg of the ping that otherwise needs a second
    /// client.
    PartyTest { arg: String },
    /// `/target [name]` — select by name (`TargetByName`, decision 0886). Resolves creatures AND
    /// players, case-insensitively, by whole name or longest common prefix, with no range, cone,
    /// liveness or hostility gate at all. `None` = bare, which the ref's `GetSlashCmdTarget` turns
    /// into your current target's name when that target is a player (a no-op re-select), and into
    /// nothing otherwise.
    Target { name: Option<String> },
    /// `/assist [name]` — select whatever the basis unit is targeting. A named basis resolves
    /// **players only** (typemask 0x10); bare assists your current target, creature or player.
    Assist { name: Option<String> },
    /// `/follow [name]` — auto-follow (decision 0890). A named subject resolves players only, and
    /// only living assistable ones (the ref's filter mode 2); bare follows the current selection,
    /// creature or player. Client-side movement — nothing goes on the wire.
    Follow { name: Option<String> },
    /// `/macrohelp` — the reference's `ChatFrame_DisplayMacroHelpText`: `MACRO_HELP_TEXT_LINE1..5`
    /// straight out of the shipped `GlobalStrings.lua` (decision 0983). Resolved in the drain,
    /// which holds the VM, so the text can never go stale against the install.
    MacroHelp,
    /// `/reload`, or `ReloadUI()` typed through `/script` — tear the UI session down and build a
    /// new one without leaving the world (decision 1291). Queued on the session seam like
    /// [`ParsedChat::Logout`]; [`crate::ui_script::run_pending_reload`] runs it at the top of
    /// the next frame. (`/console reloadUI` reaches the same request through the command
    /// registry's `reloadUI`.)
    ReloadUi,
    /// A `/console` line the engine's CVar store did not consume — a command name, or a bare
    /// CVar name to print — for the command registry ([`crate::console::execute`], decision
    /// 2303): the reference's `ConsoleCommand` table, which every subsystem registers into and
    /// which answers an unknown line by saying so.
    Console { line: String },
    /// A slash line matching neither a chat command nor an `EmotesText` name — dropped.
    Unknown,
}

/// The name half of the ref's `GetSlashCmdTarget` (ChatFrame.lua:650-658) — the argument grammar
/// **thirteen** shipped commands share, `/target` and `/assist` among them.
///
/// Its first line is a trim, not a tokenizer: `gsub(msg, "(%s*)(.*[^%s]+)(%s*)", "%2", 1)` keeps
/// **everything between the outer whitespace**, internal spaces included. This used to take the
/// first word instead, on the reasoning that 1.12 player names carry no spaces — true, but
/// `/target` resolves *creature* names too, and "Kobold Vermin" is the common case. `args` reaches
/// us already trimmed ([`parse_line`]), which is exactly what that gsub computes.
///
/// Empty defers to the caller's fallback (a bare party command acts on the selected PLAYER —
/// `target_player_name` in [`super`], the ref's `UnitIsPlayer("target")` branch).
///
/// **Not carried: the unit-token expansion.** The ref's tail expands `player`, `target`,
/// `^party[1-4]` and `^raid[0-9]` through `UnitName` before returning, so `/assist party1` means
/// "that member's name". benilla feeds only the `player` and `target` tokens to the VM's unit
/// store, so the roster half has nothing to resolve against yet; the tokens would pass through as
/// literal names. Named in decision 0886 as a gap rather than half-built here.
fn slash_target_name(args: &str) -> Option<String> {
    (!args.is_empty()).then(|| args.to_string())
}

/// Parse a trimmed slash line into an ACTION (`/join`, `/afk`, `/random`, `/wave`, the dev
/// commands…) through the boot-built command table ([`super::commands`], decision 0881). The
/// chat-TYPE commands (`/say`-family, `/w`, `/r`) never reach here on the send path —
/// [`parse_enter_type_switch`] and the reply arm consume them first; a plain no-slash line sends as
/// the box's current type ([`drain_chat_input`]). A non-slash line reaching this fn (unit tests
/// only) is [`ParsedChat::Unknown`].
///
/// The command/argument split is `ChatEdit_ParseText`'s (ChatFrame.lua l.2161-2168): the command is
/// the FIRST word, the argument is everything after that first space. That is why `/wave Bob` is a
/// `/wave` carrying an argument — before 0881 the emote arm matched the *whole* line against
/// `EmotesText`, so every emote typed with anything after it was an unknown command.
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
        // HELP_TEXT_SIMPLE's case — a command benilla registers no handler for is indistinguishable
        // from one the reference never had, which is the honest answer for both.
        None => ParsedChat::Unknown,
    }
}

/// `s` as a Lua **short** string literal, escaped — the only quoting 1.12's lexer can read for
/// arbitrary text (decision 2136).
///
/// This used to build a long-bracket literal and step its `=` level past anything the payload
/// could close early: `[=[a]]b]=]`. That works on a Lua 5.1-family lexer and **is a syntax error
/// on the 1.12 client**, whose lexer has no long-string levels at all — `read_long_string
/// 0x700010` routes `=` to its default arm as ordinary content, the `[` arm at `0x6ff771` makes
/// exactly one comparison (`cmp eax,0x5b`), and a `[=` therefore leaves the lexer holding the
/// single-character token `[`, which `prefixexp 0x6fde40` refuses with ``unexpected symbol near
/// `['`` (measured on the reference client's own Lua loader).
///
/// 2101 removed the three 5.1 grammar additions the corpus probes and left the levelled long
/// bracket standing, on a measurement of **zero occurrences** across FrameXML, GlueXML,
/// Blizzard's addons, the director's own folder and the 219-addon corpus. That measurement was
/// right and incomplete in one specific way: it scanned Lua *trees*, and this is Lua *generated
/// by Rust at runtime*, which no source scan could see and the fork's parser cannot reject at
/// compile time either. It is the one live producer of a construct the reference cannot parse,
/// and it fires whenever a `/console` argument contains `]]`.
///
/// Escaping is what 5.0 leaves: `\` and `"` are self-escaped, the three whitespace controls take
/// their named forms, and every other control byte takes a **three-digit** `\ddd` — padded so a
/// following digit cannot be swallowed into the escape.
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

/// The per-command argument grammar. Each arm is the reference handler's own body reduced to what
/// it does with `msg` — the aliases that reach it are the table's business, never this function's.
fn slash_command(index: SlashIndex, args: &str) -> ParsedChat {
    use SlashIndex as S;
    match index {
        S::Reply => ParsedChat::Reply {
            text: args.to_string(),
        },
        S::Join => {
            let (name, password) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
            if name.is_empty() {
                // CHAT_JOIN_HELP (the ref's bare-/join reply) rides the Unknown help line.
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
            // The bare joined-channel listing is P6's (it needs the list).
            None => ParsedChat::Unknown,
        },
        // **The reference's own bodies, not a second implementation** (2088). These used to build
        // a bare `ClientCommand::Chat` — which was right until the away law landed, and would now
        // be a SECOND `/afk` that sends the packet with no echo, no default substitution and no
        // mirror write. The stock parser claims a typed `/afk` before this table ever sees it, so
        // the only lines still arriving here are the ones that never touched the edit box (a
        // `WOW_PROBE_CHAT` rig's), and routing them through `SlashCmdList` funnels them into the
        // same `SendChatMessage` seam the player's own keystrokes take — one implementation, and
        // the probe exercises the real path rather than a shadow of it. Same posture, and the same
        // reasoning, as the social verbs below.
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
        // The party membership commands (decision 0434) and the duel verbs (0633): a bare command
        // falls back to the selected PLAYER (`GetSlashCmdTarget`).
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
        // The by-name selection pair (decision 0886). Both take the whole trimmed argument —
        // `/target Kobold Vermin` is one name, not a name plus junk.
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
        // The social verbs (decision 0668): the reference's own `SlashCmdList` bodies, which the
        // stock ChatFrame.lua carries since 1948 — and whose parser claims these lines before
        // they ever reach here, so these rows are the shape kept for the day the arm is pruned
        // (1948's follow-on). The argument is passed WHOLE: `/who`'s is a filter expression.
        S::Who => social_body("WHO", args),
        S::Friends => social_body("FRIENDS", args),
        S::RemoveFriend => social_body("REMOVEFRIEND", args),
        S::Ignore => social_body("IGNORE", args),
        S::Unignore => social_body("UNIGNORE", args),
        // The one-line reference bodies over globals benilla already implements (decision 0881,
        // the 0668 posture): `InitiateTrade("target")`, `InspectUnit("target")`,
        // `SetLootMethod(...)`, `RunScript(msg)`. Running the reference's own call keeps the
        // panel-opening/permission behaviour in the FrameXML that owns it.
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
        // The ref's LOOT_MASTER is a no-op without a name/target (`if GetSlashCmdTarget(msg)`).
        S::LootMaster => match slash_target_name(args) {
            Some(name) => ParsedChat::Lua {
                body: format!(
                    "SetLootMethod(\"master\", \"{}\")",
                    escape_lua_string(&name)
                ),
            },
            None => ParsedChat::Unknown,
        },
        // `/cast <name>` = the ref's `SlashCmdList["CAST"]` verbatim: `if msg ~= "" then
        // CastSpellByName(msg) end` (ChatFrame.lua:1120). The binding does the book lookup
        // (`benilla_ui::script::spellbook::resolve_spell_by_name`) and queues onto the ONE cast
        // path, so a typed `/cast` and a macro's `/cast` line are the same code — decision 0983.
        S::Cast => {
            if args.is_empty() {
                ParsedChat::Unknown
            } else {
                ParsedChat::Lua {
                    body: format!("CastSpellByName(\"{}\")", escape_lua_string(args)),
                }
            }
        }
        // `/macro` `/m` = the ref's own body: `ShowMacroFrame()` (the `RunMacro(msg)` branch beside
        // it is commented out in the shipped 1.12 ChatFrame.lua — there is no `RunMacro` binding in
        // the client at all, confirmed against the registered-binding scan).
        S::MacroUi => ParsedChat::Lua {
            body: "ShowMacroFrame()".into(),
        },
        S::MacroHelp => ParsedChat::MacroHelp,
        // `/reload` (ours, 1291) and the reference's `/console reloadUI` are the same rebuild.
        // The console command match is case-insensitive like the engine's console; `0x4035f0`
        // reads no arguments, so anything after `reloadUI` is ignored rather than an error.
        S::ReloadUi => ParsedChat::ReloadUi,
        S::ConvertRaid => ParsedChat::ConvertRaid,
        // `/errors` (ours, 1495) → the script error log. Routed as Lua rather than a new
        // `ParsedChat` variant for the same reason `/macro` is: the window is FrameXML, the
        // toggle is a FrameXML function, and a Rust arm would only forward to it.
        // `/errors clear` empties the log — the reporter's workflow is clear, reproduce,
        // screenshot, and without it the shot carries a session's worth of unrelated rows.
        S::ScriptErrors => ParsedChat::Lua {
            body: if args.trim().eq_ignore_ascii_case("clear") {
                "BenillaScriptLog_Clear()".into()
            } else {
                "BenillaScriptLog_Toggle()".into()
            },
        },
        // `/console <line>` — the stock ChatFrame.lua's handler is one line, `ConsoleExec(msg)`,
        // and a TYPED `/console` never reaches this arm: the reference's `ChatEdit_ParseText`
        // finds its own `SlashCmdList["CONSOLE"]` first (1948). What does reach it is a line
        // that skipped the edit box — a `WOW_PROBE_CHAT` rig, or a chain whose ChatFrame.lua
        // lacks the handler — and 0637's contract is that a probe line is "what the director
        // would type". So this forwards to the same verb the stock handler calls: a registered
        // CVar name writes the CVar (`fpsJournal 1`, 2008), and `reloadUI` or anything else
        // comes back through `engine_verbs` exactly as it does from the Lua route. (Before 2008
        // this arm knew `reloadui` and answered everything else "not implemented" — which is
        // what a probe's `/console fpsJournal 1` got, while the same line typed worked.)
        S::Console => ParsedChat::Lua {
            body: format!("ConsoleExec({})", lua_quoted_string(args)),
        },
        // `/script` = the ref's `RunScript(msg)`: the typed text IS the chunk, un-escaped.
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

/// Benilla's own instruments' grammar ([`DevCmd`] — no reference strings behind these).
fn dev_command(dev: DevCmd, args: &str) -> ParsedChat {
    match dev {
        // `/castvis <spell_id> [go|ground|fail]` — bare = Start (the precast hold begins), `go` =
        // the release at the selection, `ground` = the release as a pure DEST cast (a point in
        // front of the player, no unit targets — the ground-missile lane), `fail` = the reap.
        // Malformed → Unknown (dropped loudly).
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
