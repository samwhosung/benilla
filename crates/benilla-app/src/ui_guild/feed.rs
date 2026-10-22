//! The guild VM feed: resolve the roster into the snapshot the guild windows read and fire the
//! guild events on their edges. Names resolve engine-side as in the reference:
//! `GetGuildRosterInfo 0x4d1200` returns the localized class and zone names, not ids (`0x4d1355`,
//! `0x4d1391`), and the rank name from the cached guild record. The verbs act in their calls
//! ([`super::GuildSteps`]), which push the same snapshot ([`snapshot`]).

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::{GuildInfo, GuildRosterMember, GUILD_RANK_MAX_LENGTH};
use benilla_ui::script::{
    GuildMemberInfo, GuildRankInfo, GuildState as VmGuild, ScriptValue, UiScript,
};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::script_calls::VmWith;
use crate::ui_unit::{NameTables, UnitNames};

use super::{last_online, GuildLocal, GuildState, Identity, RosterRow};

/// What the feed last announced, so the events fire on edges rather than every frame.
#[derive(Default)]
pub(super) struct FedGuild {
    /// Whether this VM has had a snapshot; the first push fires the roster update, so a frame
    /// loaded after the roster still populates.
    seeded: bool,
    motd: String,
    invite: Option<(String, String)>,
}

/// The guild state the feed writes beside the VM.
type GuildVm<'w, 's> = VmWith<'w, 's, (ResMut<'static, GuildState>, ResMut<'static, GuildLocal>)>;

/// The VM's guild snapshot of `guild`, in the order `local` keeps, whose shown order it records.
/// The lazy identity fill: the reference's `GetGuildRosterInfo` asks inline on a miss with the
/// callback `0x4d1480` (`0x4d1291`), and the answer fires a bare `GUILD_ROSTER_UPDATE`; a rebuild
/// runs on a change, which stands for the stock frame's call.
pub(super) fn snapshot(
    guild: &GuildState,
    local: &mut GuildLocal,
    show_offline: i32,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
    commands: &NetCommands,
) -> VmGuild {
    let identity = guild.resolve_identity(guild.guild_id, Ask::Event(commands));
    let rows = display_rows(guild, local, show_offline, areas, identity, names);
    local.display_order = rows.iter().map(|r| r.guid).collect();
    VmGuild {
        in_guild: guild.in_guild(),
        name: identity.map(|i| i.name.clone()).unwrap_or_default(),
        rank_name: identity
            .map(|i| i.rank_name(guild.rank_index).to_string())
            .unwrap_or_default(),
        rank_index: guild.rank_index,
        // `IsGuildLeader 0x516e40`: rank 0 in a guild; a guildless rank 0 is not a leader.
        is_leader: guild.in_guild() && guild.rank_index == 0,
        rights: guild.own_rights(),
        motd: guild.motd.clone(),
        info_text: guild.info_text.clone(),
        num_members: guild.num_members(show_offline),
        roster: rows.into_iter().map(|r| r.info).collect(),
        ranks: guild
            .rank_rights
            .iter()
            .enumerate()
            .map(|(i, rights)| GuildRankInfo {
                name: identity
                    .map(|id| id.rank_name(i as u32).to_string())
                    .unwrap_or_default(),
                rights: *rights,
            })
            .collect(),
    }
}

/// Build the display snapshot, push it, and fire the guild events on their edges.
pub(super) fn feed_guild(
    mut vm: GuildVm,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    areas: Option<Res<AreaTableRes>>,
    unit_names: NameTables,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
    mut fed: Local<crate::ui_script::VmMemo<FedGuild>>,
) {
    let mut info_lines: Vec<crate::ui_action::Shown> = Vec::new();
    vm.feed(|script, (mut guild, mut local), fires| {
        let Some(script) = script else {
            return;
        };
        let fed = fed.get(script);

        // Our guild id and rank come from our own descriptor, mirrored only while the avatar is
        // streamed: the frames between logout despawn and teardown must not read as leaving the
        // guild. Their `PLAYER_GUILD_UPDATE` is the descriptor watchers' (`crate::field_watch`),
        // fired after this push.
        if let Some(store) = self_q.iter().next() {
            guild.mirror_self(store.0.player_guild_id(), store.0.player_guild_rank());
        }

        // `GUILD_INVITE_REQUEST` shows the popup. There is no hide edge: nothing in the 1.12 client
        // raises `GUILD_INVITE_CANCEL` (its wrapper `0x48f470` has no callers); the answer clears
        // the held invitation, so a second one fires again.
        if fed.invite != local.pending_invite {
            fed.invite = local.pending_invite.clone();
            if let Some((inviter, name)) = &local.pending_invite {
                fires.post(
                    "GUILD_INVITE_REQUEST",
                    vec![
                        ScriptValue::Str(inviter.clone()),
                        ScriptValue::Str(name.clone()),
                    ],
                );
            }
        }

        // Rebuilt only on a change: every apply sets the flag.
        if !guild.dirty && fed.seeded {
            return;
        }
        guild.dirty = false;
        let show_offline = script.guild_show_offline();
        script.set_guild(snapshot(
            &guild,
            &mut local,
            show_offline,
            areas.as_deref().map(|a| &a.0),
            unit_names.get(),
            &commands,
        ));

        // `GUILD_ROSTER_UPDATE`; `super::RosterUpdate` decides whether it carries arg1, whose
        // signal clears the request deadline first (`0x4d1170`).
        let first = !fed.seeded;
        fed.seeded = true;
        if let Some(kind) = guild.roster_event.take().or(first.then_some(
            // A frame loaded after the roster still needs one push to draw from.
            super::RosterUpdate::Applied,
        )) {
            if kind.arg1() {
                local.roster_allowed_at = 0.0;
                fires.post("GUILD_ROSTER_UPDATE", vec![ScriptValue::Int(1)]);
            } else {
                fires.post("GUILD_ROSTER_UPDATE", vec![]);
            }
        }

        // `GUILD_MOTD` carries the text; its chat line is the FrameXML's
        // (`ChatFrame.lua:1335-1340`).
        if fed.motd != guild.motd {
            fed.motd = guild.motd.clone();
            fires.post("GUILD_MOTD", vec![ScriptValue::Str(guild.motd.clone())]);
        }

        // `/ginfo`'s two lines, resolved here because their templates are not catalog rows.
        info_lines = std::mem::take(&mut guild.pending_info)
            .iter()
            .flat_map(|info| ginfo_lines(script, info))
            .collect();
    });
    if !info_lines.is_empty() {
        vm.run(|script| {
            if let Some(script) = script {
                crate::ui_action::show_messages(script, &mut sink, "ui_guild", info_lines);
            }
        });
    }
}

/// Ask the guilds `GetGuildInfo` missed, each with its waiter (`0x4c93d2`): the getter's lookup,
/// which the getter cannot send from inside the VM.
pub(super) fn ask_missed_guilds(
    mut vm: VmWith<ResMut<'static, GuildState>>,
    commands: Res<NetCommands>,
) {
    vm.turn(|script, mut guild| {
        let Some(script) = script else {
            return;
        };
        for (guild_id, guid) in script.take_guild_info_asks() {
            guild.wait_for_info(guild_id, guid, &commands);
        }
    });
}

/// `SMSG_GUILD_INFO`'s chat lines, `GUILD_NAME_TEMPLATE` then `GUILD_INFO_TEMPLATE` (`0x5e6fb0`,
/// at `0x5e700f` and `0x5e706b`). Not message records: the handler reads each template from the
/// script VM (`0x703bf0`) and prints chat itself, so a missing template prints nothing. It passes
/// wire fields 2, 1, 3, 4, 5 (`0x5e704d`-`0x5e7061`): month first, where the wire has day first.
fn ginfo_lines(script: &UiScript, info: &GuildInfo) -> Vec<crate::ui_action::Shown> {
    use benilla_ui::strings::Arg;
    let get = |key: &str| script.lua().globals().get::<String>(key).ok();
    let line = |template: Option<String>, args: &[Arg<'_>]| {
        let text = benilla_ui::strings::fill(&template?, args);
        (!text.is_empty())
            .then(|| crate::ui_action::Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, text))
    };
    let d = |v: u32| Arg::D(i64::from(v));
    [
        line(get("GUILD_NAME_TEMPLATE"), &[Arg::S(&info.name)]),
        line(
            get("GUILD_INFO_TEMPLATE"),
            &[
                d(info.created_month),
                d(info.created_day),
                d(info.created_year),
                d(info.member_count),
                d(info.account_count),
            ],
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The roster in display order: every member, sorted, never filtered. Until the guild query
/// answers, only the rank-name column is empty.
pub(super) fn display_rows(
    guild: &GuildState,
    local: &GuildLocal,
    show_offline: i32,
    areas: Option<&AreaTableCatalog>,
    identity: Option<&Identity>,
    names: UnitNames<'_>,
) -> Vec<RosterRow> {
    let mut rows: Vec<RosterRow> = guild
        .members
        .iter()
        .map(|m| row(m, areas, identity, names))
        .collect();
    local.sort.order(&mut rows, show_offline != 0);
    rows
}

/// One roster row, ids resolved. An offline member keeps level, class and zone, which the wire
/// carries for every member, so the pane greys the row rather than blanking it.
fn row(
    m: &GuildRosterMember,
    areas: Option<&AreaTableCatalog>,
    identity: Option<&Identity>,
    names: UnitNames<'_>,
) -> RosterRow {
    let online = m.is_online();
    RosterRow {
        guid: m.guid,
        last_online_days: m.last_online_days,
        info: GuildMemberInfo {
            guid: m.guid,
            name: m.name.clone(),
            rank: identity
                .map(|i| i.rank_name(m.rank_id).to_string())
                .unwrap_or_default(),
            rank_index: m.rank_id,
            level: u32::from(m.level),
            class: names
                .class(m.class)
                .map(|(display, _)| display.to_string())
                .unwrap_or_default(),
            zone: areas
                .and_then(|a| a.name(m.zone))
                .unwrap_or_default()
                .to_string(),
            note: m.public_note.clone(),
            officer_note: m.officer_note.clone(),
            online,
            presence: m.presence,
            // No float on the wire for an online member; the formatter reads all zeroes as nil.
            last_online: if online {
                Default::default()
            } else {
                last_online(m.last_online_days)
            },
        },
    }
}

/// The rank name, or `None` past vmangos's cap of 15, where it kicks the session
/// (`Handlers/GuildHandler.cpp:580-584`, `:600-604`); the stock edit box stops at 15. Deviation: a
/// script's 16-letter name, which the reference's bindings pass (1 to 16 units, `0x4d20d0`,
/// `0x4d2210`), is dropped rather than sent or truncated, because vmangos would kick the session.
pub(super) fn capped_rank_name(name: String) -> Option<String> {
    if name.chars().count() > GUILD_RANK_MAX_LENGTH {
        warn!(
            "guild: refusing a {}-character rank name (the server kicks over {GUILD_RANK_MAX_LENGTH})",
            name.chars().count()
        );
        return None;
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::guild_presence;

    #[test]
    fn online_is_the_whole_byte() {
        let member = |presence| GuildRosterMember {
            presence,
            ..Default::default()
        };
        assert!(member(guild_presence::ONLINE).is_online());
        assert!(member(guild_presence::AFK).is_online(), "no ONLINE bit set");
        assert!(member(0x80).is_online(), "an unknown bit reads as online");
        assert!(!member(guild_presence::OFFLINE).is_online());
    }

    #[test]
    fn an_offline_row_keeps_its_level_class_and_zone() {
        let member = GuildRosterMember {
            guid: 5,
            presence: guild_presence::OFFLINE,
            name: "Kaplan".into(),
            rank_id: 1,
            level: 42,
            class: 8, // Mage
            zone: 12,
            last_online_days: 3.5,
            public_note: "alt".into(),
            officer_note: String::new(),
        };
        let korean = crate::ui_unit::TestNames::korean();
        let row = row(&member, None, None, korean.names());
        assert_eq!(row.info.level, 42);
        assert_eq!(
            row.info.class, "마법사",
            "the table's word at the install's locale"
        );
        assert!(!row.info.online);
        assert_eq!(row.info.last_online.days, 3);
        assert_eq!(row.info.presence, guild_presence::OFFLINE);
        assert_eq!(row.guid, 5);
    }

    #[test]
    fn an_online_row_reports_no_last_online() {
        let member = GuildRosterMember {
            presence: guild_presence::ONLINE,
            // Ignored for an online member, as in the reference (`0x4d14fd`).
            last_online_days: 9.0,
            ..Default::default()
        };
        assert_eq!(
            row(&member, None, None, Default::default())
                .info
                .last_online,
            Default::default()
        );
    }

    #[test]
    fn an_over_long_rank_name_is_refused() {
        assert_eq!(
            capped_rank_name("Officer".into()).as_deref(),
            Some("Officer")
        );
        assert!(capped_rank_name("A".repeat(GUILD_RANK_MAX_LENGTH)).is_some());
        assert_eq!(
            capped_rank_name("A".repeat(GUILD_RANK_MAX_LENGTH + 1)),
            None
        );
    }
}
