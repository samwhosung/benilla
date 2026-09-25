//! The slash-command table: every `/command` the client answers, keyed by the aliases the
//! player's own strings define, read off the UI VM's globals the way `ChatEdit_ParseText` walks
//! them (`ChatFrame.lua:2164-2200`): `SLASH_<INDEX><n>` for the actions, `EMOTE<i>_CMD<j>` →
//! `EMOTE<i>_TOKEN` for the emotes. An action wins over an emote alias of the same name, as the
//! reference tries `SlashCmdList` first; benilla's own commands, literals, go in after both.

use std::collections::HashMap;

use bevy::prelude::*;

/// A command benilla implements, named by its `SlashCmdList` index, the `SLASH_<INDEX><n>` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashIndex {
    Reply,
    Join,
    Leave,
    ListChannel,
    ChatAfk,
    ChatDnd,
    Random,
    Played,
    Help,
    Logout,
    Quit,
    Invite,
    Uninvite,
    Promote,
    Duel,
    DuelCancel,
    Pvp,
    Who,
    Friends,
    RemoveFriend,
    Ignore,
    Unignore,
    Trade,
    Inspect,
    Script,
    LootFfa,
    LootRoundRobin,
    LootMaster,
    Target,
    Assist,
    Follow,
    /// `/cast <name>`: the reference's handler calls `CastSpellByName(msg)` (`ChatFrame.lua:1120`).
    Cast,
    /// `/macro`, `/m`: the reference's `ShowMacroFrame()`.
    MacroUi,
    /// `/macrohelp`: the reference's five-line help text.
    MacroHelp,
    /// `/console <line>`: `ConsoleExec(msg)`, as the reference's handler (`ChatFrame.lua:671`).
    Console,
    /// `/reload`, the rebuild `/console reloadUI` runs. Deviation: 1.12 has no such alias (later
    /// clients added it); kept so a player can reload the interface after toggling addons.
    ReloadUi,
    /// `/errors`, `/err`: opens benilla's script error log. Deviation: 1.12 has only the
    /// `ScriptErrors` modal, which shows a burst's first error and keeps none; addon users need
    /// the list, so it is in every build.
    ScriptErrors,
    /// `/convertraid`: convert the party to a raid (`CMSG_GROUP_RAID_CONVERT`, leader only).
    /// Deviation: 1.12's only trigger is the Raid tab's Convert button (`RaidFrame.xml`).
    ConvertRaid,
}

impl SlashIndex {
    /// The reference's index name, the `SLASH_<KEY><n>` prefix its aliases live under.
    fn key(self) -> &'static str {
        match self {
            Self::Reply => "REPLY",
            Self::Join => "JOIN",
            Self::Leave => "LEAVE",
            Self::ListChannel => "LIST_CHANNEL",
            Self::ChatAfk => "CHAT_AFK",
            Self::ChatDnd => "CHAT_DND",
            Self::Random => "RANDOM",
            Self::Played => "PLAYED",
            Self::Help => "HELP",
            Self::Logout => "LOGOUT",
            Self::Quit => "QUIT",
            Self::Invite => "INVITE",
            Self::Uninvite => "UNINVITE",
            Self::Promote => "PROMOTE",
            Self::Duel => "DUEL",
            Self::DuelCancel => "DUEL_CANCEL",
            Self::Pvp => "PVP",
            Self::Who => "WHO",
            Self::Friends => "FRIENDS",
            Self::RemoveFriend => "REMOVEFRIEND",
            Self::Ignore => "IGNORE",
            Self::Unignore => "UNIGNORE",
            Self::Trade => "TRADE",
            Self::Inspect => "INSPECT",
            Self::Script => "SCRIPT",
            Self::LootFfa => "LOOT_FFA",
            Self::LootRoundRobin => "LOOT_ROUNDROBIN",
            Self::LootMaster => "LOOT_MASTER",
            Self::Target => "TARGET",
            Self::Assist => "ASSIST",
            Self::Follow => "FOLLOW",
            Self::Cast => "CAST",
            Self::MacroUi => "MACRO",
            Self::MacroHelp => "MACROHELP",
            Self::Console => "CONSOLE",
            // No shipped strings under these three keys: their aliases are literals in `build`.
            Self::ReloadUi => "RELOADUI",
            Self::ScriptErrors => "BENILLASCRIPTERRORS",
            Self::ConvertRaid => "BENILLACONVERTRAID",
        }
    }

    /// Every registered index; any other command answers `HELP_TEXT_SIMPLE`, as unknown ones do.
    const ALL: [Self; 38] = [
        Self::Reply,
        Self::Join,
        Self::Leave,
        Self::ListChannel,
        Self::ChatAfk,
        Self::ChatDnd,
        Self::Random,
        Self::Played,
        Self::Help,
        Self::Logout,
        Self::Quit,
        Self::Invite,
        Self::Uninvite,
        Self::Promote,
        Self::Duel,
        Self::DuelCancel,
        Self::Pvp,
        Self::Who,
        Self::Friends,
        Self::RemoveFriend,
        Self::Ignore,
        Self::Unignore,
        Self::Trade,
        Self::Inspect,
        Self::Script,
        Self::LootFfa,
        Self::LootRoundRobin,
        Self::LootMaster,
        Self::Target,
        Self::Assist,
        Self::Follow,
        Self::Cast,
        Self::MacroUi,
        Self::MacroHelp,
        Self::Console,
        Self::ReloadUi,
        Self::ScriptErrors,
        Self::ConvertRaid,
    ];
}

/// benilla's own diagnostic commands, in dev builds only; their aliases are literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DevCmd {
    CastVis,
    ChatTest,
    PartyTest,
    Shot,
    Liquid,
    Reaction,
}

impl DevCmd {
    fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::CastVis => &["castvis"],
            Self::ChatTest => &["chattest"],
            Self::PartyTest => &["partytest"],
            Self::Shot => &["shot"],
            Self::Liquid => &["liquid"],
            Self::Reaction => &["reaction", "react"],
        }
    }

    const ALL: [Self; 6] = [
        Self::CastVis,
        Self::ChatTest,
        Self::PartyTest,
        Self::Shot,
        Self::Liquid,
        Self::Reaction,
    ];
}

/// What a typed `/command` resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// A reference `SlashCmdList` command benilla implements.
    Slash(SlashIndex),
    /// An emote alias, with its token's `EmotesText` id (`/lol` → LAUGH → 45).
    Emote { text_id: u32 },
    /// One of benilla's own instruments.
    Dev(DevCmd),
}

/// The built table: alias (lowercase, no leading `/`) → command.
#[derive(Resource, Default)]
pub(crate) struct SlashCommands {
    by_alias: HashMap<String, Command>,
    /// Aliases per source, logged at boot: `(slash, emote, benilla additions, dev instruments)`.
    counts: (usize, usize, usize, usize),
}

impl SlashCommands {
    /// Resolve a typed command (no leading `/`, any case).
    pub(crate) fn lookup(&self, cmd: &str) -> Option<Command> {
        self.by_alias.get(&cmd.to_ascii_lowercase()).copied()
    }

    /// Build from `get`, a global-string lookup, and `text_id`, an `EmotesText` name → id map.
    pub(crate) fn build(
        get: impl Fn(&str) -> Option<String>,
        text_id: impl Fn(&str) -> Option<u32>,
    ) -> Self {
        let mut by_alias: HashMap<String, Command> = HashMap::new();

        // 1 · the actions: `SLASH_<INDEX><n>` from 1 to the first gap, as `while cmdString` walks.
        for index in SlashIndex::ALL {
            for n in 1.. {
                let Some(alias) = get(&format!("SLASH_{}{n}", index.key())) else {
                    break;
                };
                insert(&mut by_alias, &alias, Command::Slash(index));
            }
        }
        let slash_aliases = by_alias.len();

        // 2 · the emotes, `EMOTE<i>_CMD<j>` → `EMOTE<i>_TOKEN` → id, to the first missing `CMD1`.
        for i in 1.. {
            let Some(first) = get(&format!("EMOTE{i}_CMD1")) else {
                break;
            };
            // A token with no `EmotesText` row (EMOTE27 is "UNUSED") has nothing to send.
            let resolved = get(&format!("EMOTE{i}_TOKEN")).and_then(|t| text_id(&t));
            if let Some(text_id) = resolved {
                insert(&mut by_alias, &first, Command::Emote { text_id });
                for j in 2.. {
                    let Some(alias) = get(&format!("EMOTE{i}_CMD{j}")) else {
                        break;
                    };
                    insert(&mut by_alias, &alias, Command::Emote { text_id });
                }
            }
        }
        let emote_aliases = by_alias.len() - slash_aliases;

        // 3 · benilla's player-facing additions, in every build; after the walks, so shipped wins.
        insert(
            &mut by_alias,
            "reload",
            Command::Slash(SlashIndex::ReloadUi),
        );
        for alias in ["errors", "err"] {
            insert(
                &mut by_alias,
                alias,
                Command::Slash(SlashIndex::ScriptErrors),
            );
        }
        insert(
            &mut by_alias,
            "convertraid",
            Command::Slash(SlashIndex::ConvertRaid),
        );
        let added_aliases = by_alias.len() - slash_aliases - emote_aliases;

        // 4 · the dev instruments, dev builds only; a player build answers them as unknown.
        let before_dev = by_alias.len();
        if crate::run_mode::dev_affordances() {
            for dev in DevCmd::ALL {
                for alias in dev.aliases() {
                    insert(&mut by_alias, alias, Command::Dev(dev));
                }
            }
        }
        let dev_aliases = by_alias.len() - before_dev;

        Self {
            by_alias,
            counts: (slash_aliases, emote_aliases, added_aliases, dev_aliases),
        }
    }

    /// Distinct aliases per source (the shipped strings repeat: `EMOTE87_CMD1` and `_CMD2` are
    /// both `"/sit"`); a player build reports 0 dev aliases.
    pub(super) fn counts(&self) -> (usize, usize, usize, usize) {
        self.counts
    }
}

/// The first source to claim an alias keeps it: the reference's pass order, made static.
fn insert(map: &mut HashMap<String, Command>, alias: &str, cmd: Command) {
    let key = alias.trim().trim_start_matches('/').to_ascii_lowercase();
    if !key.is_empty() {
        map.entry(key).or_insert(cmd);
    }
}

/// Build the table in `PostStartup`, after `Startup` has inserted the VM and the emote catalog.
pub(crate) fn build_slash_commands(
    mut commands: Commands,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
    emotes: Option<Res<crate::sound::EmoteSounds>>,
) {
    let (Some(script), Some(emotes)) = (script, emotes) else {
        warn!("chat: no UI VM or emote catalog — the slash-command table is EMPTY");
        commands.insert_resource(SlashCommands::default());
        return;
    };
    let globals = script.lua().globals();
    let table = SlashCommands::build(
        |name| globals.get::<String>(name).ok().filter(|s| !s.is_empty()),
        |token| emotes.text_id(token),
    );
    let (slash, emote, added, dev) = table.counts();
    // The shipped 1.12 data gives the counts `real_alias_table_resolves_the_shipped_commands` pins.
    info!(
        "chat: slash table — {slash} command aliases, {emote} emote aliases, \
         {added} benilla additions, {dev} instrument aliases"
    );
    if emote == 0 {
        error!("chat: NO emote aliases — every /wave-style command is dead (GlobalStrings?)");
    }
    commands.insert_resource(table);
}
