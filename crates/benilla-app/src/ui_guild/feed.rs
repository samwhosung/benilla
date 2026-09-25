//! The guild VM feed and drain: resolve the roster into the snapshot the guild windows read, fire
//! the guild events on their edges, and send the Lua [`GuildRequest`] intents. Names resolve
//! engine-side as in the reference: `GetGuildRosterInfo 0x4d1200` returns the localized class and
//! zone names, not ids (`0x4d1355`, `0x4d1391`), and the rank name from the cached guild record.

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::{
    guild_presence, GuildInfo, GuildRosterMember, GUILD_RANKS_MAX_COUNT, GUILD_RANKS_MIN_COUNT,
    GUILD_RANK_MAX_LENGTH,
};
use benilla_ui::script::{
    GuildMemberInfo, GuildRankInfo, GuildRequest, GuildState as VmGuild, ScriptValue, UiScript,
};
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_unit::class_names;

use super::{last_online, GuildState, Identity, RosterRow, ROSTER_REQUEST_THROTTLE_SECS};

/// `CHAT_FLAG_AFK` and `CHAT_FLAG_DND` (`GlobalStrings.lua:534-535`), which the reference loads at
/// `0x4d1404` and `0x4d13ed`.
const CHAT_FLAG_AFK: &str = "<AFK>";
const CHAT_FLAG_DND: &str = "<DND>";

/// What the feed last announced, so the events fire on edges rather than every frame.
#[derive(Default)]
pub(super) struct FedGuild {
    /// Whether this VM has had a snapshot; the first push fires the roster update, so a frame
    /// loaded after the roster still populates.
    seeded: bool,
    motd: String,
    invite: Option<(String, String)>,
}

/// Build the display snapshot, push it, and fire the guild events on their edges.
pub(super) fn feed_guild(
    script: Option<NonSendMut<UiScript>>,
    mut guild: ResMut<GuildState>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    areas: Option<Res<AreaTableRes>>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
    mut fed: Local<crate::ui_script::VmMemo<FedGuild>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let fed = fed.get(&script);

    // Our guild id and rank come from our own descriptor, mirrored only while the avatar is
    // streamed: the frames between logout despawn and teardown must not read as leaving the guild.
    let mut id_changed = false;
    let mut rank_changed = false;
    if let Some(store) = self_q.iter().next() {
        let (id, rank) = (store.0.player_guild_id(), store.0.player_guild_rank());
        id_changed = id != guild.guild_id;
        rank_changed = rank != guild.rank_index;
        guild.mirror_self(id, rank);
    }

    // Rebuilt only on a change: every apply and every local intent sets the flag.
    if !guild.dirty && fed.seeded {
        return;
    }
    guild.dirty = false;

    // The lazy identity fill: the reference's `GetGuildRosterInfo` asks inline on a miss, and the
    // answer fires a bare `GUILD_ROSTER_UPDATE`.
    let guild_id = guild.guild_id;
    guild.request_identity(guild_id, &commands);
    let identity = guild.identity(guild_id).cloned();
    let identity = identity.as_ref();

    let rows = display_rows(&guild, areas.as_deref().map(|a| &a.0), identity);
    let display_order: Vec<u64> = rows.iter().map(|r| r.guid).collect();
    let selection = index_of(&display_order, guild.selection);
    guild.display_order = display_order;

    script.set_guild(VmGuild {
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
        num_members: guild.num_members(),
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
        selection,
        show_offline: guild.show_offline,
    });

    // `GUILD_ROSTER_UPDATE`; `super::RosterUpdate` decides whether it carries arg1.
    let first = !fed.seeded;
    fed.seeded = true;
    if let Some(kind) = guild.roster_event.take().or(first.then_some(
        // A frame loaded after the roster still needs one push to draw from.
        super::RosterUpdate::Applied,
    )) {
        let args = if kind.arg1() {
            vec![ScriptValue::Int(1)]
        } else {
            Vec::new()
        };
        script.fire_event("GUILD_ROSTER_UPDATE", args);
    }

    // `GUILD_MOTD` carries the text; its chat line is the FrameXML's (`ChatFrame.lua:1335-1340`).
    if fed.motd != guild.motd {
        fed.motd = guild.motd.clone();
        script.fire_event("GUILD_MOTD", vec![ScriptValue::Str(guild.motd.clone())]);
    }

    // `PLAYER_GUILD_UPDATE`, as the reference's descriptor callbacks fire it (`0x5e27b0`,
    // `0x5e2770`, `0x515e50`): bare on a `PLAYER_GUILDRANK` change, and once per unit token naming
    // the player, the token as arg1, on a `PLAYER_GUILDID` change. Only our own avatar's edges
    // fire here; other players' are not built, and the reference's per-token walk is unconfirmed.
    if id_changed {
        script.fire_event(
            "PLAYER_GUILD_UPDATE",
            vec![ScriptValue::Str("player".to_string())],
        );
    }
    if rank_changed {
        script.fire_event("PLAYER_GUILD_UPDATE", Vec::new());
    }

    // `GUILD_INVITE_REQUEST` shows the popup. There is no hide edge: nothing in the 1.12 client
    // raises `GUILD_INVITE_CANCEL` (its wrapper `0x48f470` has no callers).
    if fed.invite != guild.pending_invite {
        fed.invite = guild.pending_invite.clone();
        if let Some((inviter, name)) = &guild.pending_invite {
            script.fire_event(
                "GUILD_INVITE_REQUEST",
                vec![
                    ScriptValue::Str(inviter.clone()),
                    ScriptValue::Str(name.clone()),
                ],
            );
        }
    }

    // `/ginfo`'s two lines, resolved here because their templates are not catalog rows.
    let info_lines: Vec<crate::ui_action::Shown> = std::mem::take(&mut guild.pending_info)
        .iter()
        .flat_map(|info| ginfo_lines(&script, info))
        .collect();
    if !info_lines.is_empty() {
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_guild", info_lines);
    }
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
    areas: Option<&AreaTableCatalog>,
    identity: Option<&Identity>,
) -> Vec<RosterRow> {
    let mut rows: Vec<RosterRow> = guild
        .members
        .iter()
        .map(|m| row(m, areas, identity))
        .collect();
    guild.sort.order(&mut rows, guild.show_offline);
    rows
}

/// One roster row, ids resolved. An offline member keeps level, class and zone, which the wire
/// carries for every member, so the pane greys the row rather than blanking it.
fn row(
    m: &GuildRosterMember,
    areas: Option<&AreaTableCatalog>,
    identity: Option<&Identity>,
) -> RosterRow {
    let online = m.is_online();
    RosterRow {
        guid: m.guid,
        last_online_days: m.last_online_days,
        info: GuildMemberInfo {
            name: m.name.clone(),
            rank: identity
                .map(|i| i.rank_name(m.rank_id).to_string())
                .unwrap_or_default(),
            rank_index: m.rank_id,
            level: u32::from(m.level),
            class: class_names(m.class)
                .map(|(display, _)| display.to_string())
                .unwrap_or_default(),
            zone: areas
                .and_then(|a| a.name(m.zone))
                .unwrap_or_default()
                .to_string(),
            note: m.public_note.clone(),
            officer_note: m.officer_note.clone(),
            online,
            status: status_tag(m.presence).to_string(),
            // No float on the wire for an online member; the formatter reads all zeroes as nil.
            last_online: if online {
                Default::default()
            } else {
                last_online(m.last_online_days)
            },
        },
    }
}

/// The away tag, `GetGuildRosterInfo`'s tenth return: always a string, `""` rather than nil. DND
/// is tested before AFK (`0x4d13e4`, `0x4d13fb`), so a member flagged both shows as DND.
pub(super) fn status_tag(presence: u8) -> &'static str {
    if presence & guild_presence::DND != 0 {
        CHAT_FLAG_DND
    } else if presence & guild_presence::AFK != 0 {
        CHAT_FLAG_AFK
    } else {
        ""
    }
}

/// The 1-based row of a guid in the shown order, `0` when absent, as `GetGuildRosterSelection
/// 0x4d1890` searches it (`0x4d1030`): a member who left reads as nothing selected.
pub(super) fn index_of(order: &[u64], guid: u64) -> u32 {
    if guid == 0 {
        return 0;
    }
    order
        .iter()
        .position(|g| *g == guid)
        .map_or(0, |i| i as u32 + 1)
}

/// Turn the Lua guild intents into their sends. The note verbs carry a 1-based display row where
/// the wire wants a name; select, sort and show-offline are local and send nothing.
pub(super) fn drain_guild(
    script: Option<NonSendMut<UiScript>>,
    mut guild: ResMut<GuildState>,
    commands: Res<NetCommands>,
    time: Res<Time<Real>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_guild_requests();
    if requests.is_empty() {
        return;
    }
    for request in requests {
        match request {
            GuildRequest::Roster => {
                // One request per 10 s, the rest swallowed silently (`0x4d10d0`, against
                // `0xb73130`). Real time: the virtual clock's 250 ms `max_delta` lags in a hitch.
                let now = time.elapsed_secs_f64();
                if now >= guild.roster_allowed_at {
                    guild.roster_allowed_at = now + ROSTER_REQUEST_THROTTLE_SECS;
                    let _ = commands.0.send(ClientCommand::GuildRosterRequest);
                }
            }
            GuildRequest::Info => {
                let _ = commands.0.send(ClientCommand::GuildInfoRequest);
            }
            GuildRequest::Invite(name) => {
                let _ = commands.0.send(ClientCommand::GuildInvite { name });
            }
            GuildRequest::Uninvite(name) => {
                let _ = commands.0.send(ClientCommand::GuildRemove { name });
            }
            GuildRequest::Promote(name) => {
                let _ = commands.0.send(ClientCommand::GuildPromote { name });
            }
            GuildRequest::Demote(name) => {
                let _ = commands.0.send(ClientCommand::GuildDemote { name });
            }
            GuildRequest::SetLeader(name) => {
                let _ = commands.0.send(ClientCommand::GuildLeader { name });
            }
            GuildRequest::Accept => {
                guild.clear_invite();
                let _ = commands.0.send(ClientCommand::GuildAccept);
            }
            GuildRequest::Decline => {
                guild.clear_invite();
                let _ = commands.0.send(ClientCommand::GuildDecline);
            }
            GuildRequest::Leave => {
                let _ = commands.0.send(ClientCommand::GuildLeave);
            }
            GuildRequest::Disband => {
                let _ = commands.0.send(ClientCommand::GuildDisband);
            }
            GuildRequest::SetMotd(motd) => {
                let _ = commands.0.send(ClientCommand::GuildMotd { motd });
            }
            GuildRequest::SetInfoText(text) => {
                // No local write: `SetGuildInfoText 0x4d2380` only sends, leaving `0xb72720` to
                // change with the next roster.
                let _ = commands.0.send(ClientCommand::GuildInfoText { text });
            }
            GuildRequest::SetPublicNote { index, note } => {
                set_note(&mut guild, &commands, index, note, false);
            }
            GuildRequest::SetOfficerNote { index, note } => {
                set_note(&mut guild, &commands, index, note, true);
            }
            GuildRequest::SaveRank {
                rank_index,
                rights,
                name,
            } => {
                if let Some(name) = capped_rank_name(name) {
                    let _ = commands.0.send(ClientCommand::GuildRank {
                        rank_id: rank_index,
                        rights,
                        name,
                    });
                }
            }
            GuildRequest::AddRank(name) => {
                // Ten ranks at most, and more than five to remove one, as the reference's
                // `GuildControlAddRank` (`0x4d2271`) and `GuildControlDelRank` (`0x4d22e6`) check
                // silently; the stock buttons follow them (`FriendsFrame.lua:887`, `:908`).
                if guild.rank_rights.len() < GUILD_RANKS_MAX_COUNT {
                    if let Some(name) = capped_rank_name(name) {
                        let _ = commands.0.send(ClientCommand::GuildAddRank { name });
                    }
                }
            }
            GuildRequest::DelRank => {
                if guild.rank_rights.len() > GUILD_RANKS_MIN_COUNT {
                    let _ = commands.0.send(ClientCommand::GuildDelRank);
                }
            }
            GuildRequest::Select(index) => guild.select(index),
            GuildRequest::SetShowOffline(on) => guild.set_show_offline(on),
            GuildRequest::Sort(field) => guild.sort_by(&field),
        }
    }
}

/// A note verb: resolve the row to the member, write the note locally, and send only if it
/// changed, as the reference gates the send on its own write (`0x4d15e0`, `0x64a480`).
fn set_note(
    guild: &mut GuildState,
    commands: &NetCommands,
    index: u32,
    note: String,
    officer: bool,
) {
    let Some(member) = guild.member_at_mut(index) else {
        return;
    };
    let slot = if officer {
        &mut member.officer_note
    } else {
        &mut member.public_note
    };
    if *slot == note {
        return;
    }
    *slot = note.clone();
    let name = member.name.clone();
    guild.dirty = true;
    let command = if officer {
        ClientCommand::GuildSetOfficerNote { name, note }
    } else {
        ClientCommand::GuildSetPublicNote { name, note }
    };
    let _ = commands.0.send(command);
}

/// The rank name, or `None` past vmangos's cap of 15, where it kicks the session
/// (`Handlers/GuildHandler.cpp:580-584`, `:600-604`); the stock edit box stops at 15. Deviation: a
/// script's 16-letter name, which the reference's bindings pass (1 to 16 units, `0x4d20d0`,
/// `0x4d2210`), is dropped rather than sent or truncated, because vmangos would kick the session.
fn capped_rank_name(name: String) -> Option<String> {
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

    #[test]
    fn dnd_wins_over_afk_and_the_tag_is_always_a_string() {
        use guild_presence::{AFK, DND, OFFLINE, ONLINE};
        assert_eq!(status_tag(ONLINE | AFK | DND), "<DND>");
        assert_eq!(status_tag(ONLINE | DND), "<DND>");
        assert_eq!(status_tag(ONLINE | AFK), "<AFK>");
        assert_eq!(status_tag(ONLINE), "");
        assert_eq!(status_tag(OFFLINE), "", "never nil, even offline");
    }

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
        let row = row(&member, None, None);
        assert_eq!(row.info.level, 42);
        assert_eq!(row.info.class, "Mage");
        assert!(!row.info.online);
        assert_eq!(row.info.last_online.days, 3);
        assert_eq!(row.info.status, "");
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
            row(&member, None, None).info.last_online,
            Default::default()
        );
    }

    #[test]
    fn the_shown_order_maps_both_ways() {
        assert_eq!(index_of(&[11, 22, 33], 22), 2);
        assert_eq!(index_of(&[22, 11, 33], 22), 1, "same player, new row");
        assert_eq!(index_of(&[11, 33], 22), 0, "no longer listed");
        assert_eq!(index_of(&[11, 22], 0), 0, "nothing selected");
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
