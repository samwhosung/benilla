//! The guild VM feed/drain — the systems half of [`super`]: resolve the wire's ids into the
//! display-ready snapshot the guild windows read, fire the events on their edges, and turn the
//! Lua-side [`GuildRequest`] intents into their sends.
//!
//! Everything resolved here is resolved *engine-side in the reference too*:
//! `GetGuildRosterInfo 0x4d1200` hands Lua the localized `ChrClasses` and `AreaTable` names, not
//! the ids (`0x4d1355`, `0x4d1391`), and the rank name out of the cached guild record — the house
//! law from `script/social.rs:8-13`. Unlike the friend list and `/who`, the guild roster carries
//! level, class and zone for **offline** members as well, so those columns stay populated and
//! merely grey out.

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

/// The `<AFK>`/`<DND>` tags, `CHAT_FLAG_AFK`/`CHAT_FLAG_DND` (`GlobalStrings.lua:766-767`) — the
/// same pair the friend list and the chat frame use, which is what the reference loads at
/// `0x4d1404`/`0x4d13ed`.
const CHAT_FLAG_AFK: &str = "<AFK>";
const CHAT_FLAG_DND: &str = "<DND>";

/// What the feed last announced, so the events fire on edges rather than every frame.
#[derive(Default)]
pub(super) struct FedGuild {
    /// Has the VM been given a snapshot at all yet? The first push fires the roster update, so a
    /// frame loaded after the roster arrived still populates.
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

    // Our own guild id and rank live in our own descriptor, not in any guild packet, so they are
    // mirrored per frame from it. Only when the avatar is actually streamed: a frame between the
    // logout despawn and the disconnect teardown must not read as "left the guild".
    let mut id_changed = false;
    let mut rank_changed = false;
    if let Some(store) = self_q.iter().next() {
        let (id, rank) = (store.0.player_guild_id(), store.0.player_guild_rank());
        id_changed = id != guild.guild_id;
        rank_changed = rank != guild.rank_index;
        guild.mirror_self(id, rank);
    }

    // Rebuilding a large roster costs a string clone per column per member, so it happens on a
    // change rather than per frame — every apply and every local intent sets the flag.
    if !guild.dirty && fed.seeded {
        return;
    }
    guild.dirty = false;

    // The lazy identity fill (`super`'s module doc): asking here is what the reference's
    // `GetGuildRosterInfo` does inline on a cache miss, and the answer arrives as its own
    // no-arg GUILD_ROSTER_UPDATE.
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
        // `IsGuildLeader 0x516e40` — rank 0 IS the guild master, and being guildless is not being
        // one, however the rank field happens to read.
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

    // GUILD_ROSTER_UPDATE, and whether it carries arg1 — the whole of that decision is
    // `super::RosterUpdate`, including why the number 1 rather than a boolean.
    let first = !fed.seeded;
    fed.seeded = true;
    if let Some(kind) = guild.roster_event.take().or(first.then_some(
        // A frame that loads after the roster did still needs one push to draw from.
        super::RosterUpdate::Applied,
    )) {
        let args = if kind.arg1() {
            vec![ScriptValue::Int(1)]
        } else {
            Vec::new()
        };
        script.fire_event("GUILD_ROSTER_UPDATE", args);
    }

    // GUILD_MOTD carries the text itself; the chat line it prints is the FrameXML's
    // (`ChatFrame.lua:1335-1340`), which is why nothing composes one here.
    if fed.motd != guild.motd {
        fed.motd = guild.motd.clone();
        script.fire_event("GUILD_MOTD", vec![ScriptValue::Str(guild.motd.clone())]);
    }

    // PLAYER_GUILD_UPDATE. Two of the reference's three paths are descriptor field callbacks:
    // PLAYER_GUILDRANK's fires with **no arguments** for any player object, and PLAYER_GUILDID's
    // fires **once per unit token** naming that player, with the token as arg1
    // (`0x5e27b0` / `0x5e2770` → `0x515e50`). Ours covers our own avatar, which is every consumer
    // 1.12's FrameXML has — `InGuildCheck` and the paper doll both read `GetGuildInfo("player")`
    // and ignore the argument. The per-token broadcast for OTHER units is deliberately not built:
    // it has no consumer here, and the broadcaster's per-token walk is the one part of that path
    // that is inferred, unconfirmed in the binary, so it is not something to build on yet.
    if id_changed {
        script.fire_event(
            "PLAYER_GUILD_UPDATE",
            vec![ScriptValue::Str("player".to_string())],
        );
    }
    if rank_changed {
        script.fire_event("PLAYER_GUILD_UPDATE", Vec::new());
    }

    // GUILD_INVITE_REQUEST arms the popup. There is **no** hide edge: `GUILD_INVITE_CANCEL`'s
    // wrapper `0x48f470` has zero callers image-wide, so nothing in 1.12 raises it and neither do
    // we — the FrameXML registers for it and simply never hears it.
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

/// `SMSG_GUILD_INFO`'s two chat lines — `GUILD_NAME_TEMPLATE` then `GUILD_INFO_TEMPLATE`, in the
/// order `0x5e6fb0` emits them (`0x5e700f`, then `0x5e706b`).
///
/// **Neither is a message record**, so neither can ride the by-key queue: the handler resolves each
/// token through the script VM (`0x703bf0`) and formats it into a chat line itself, with no `kind`
/// and no sound to read. `MsgKind::Chat` here is therefore the handler's own surface rather than a
/// catalog lookup, and the absent key is silent — the same GlobalStrings data-suppression the keyed
/// route wears (decision 2054).
///
/// **The date is month-day-year and the wire is day-month-year.** The handler's cdecl pushes
/// (`0x5e704d`–`0x5e7061`) place the arguments **wire#2, wire#1, wire#3, wire#4, wire#5** — the
/// first two swapped — which is exactly what an enUS `m-d-y` template does to vmangos's
/// `createdDay, createdMonth, createdYear` order.
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

/// The roster in display order — **every** member, sorted, never filtered (see [`super::sort`]).
///
/// The rank name comes from the guild's identity, which is why an unanswered query leaves that one
/// column empty rather than the whole row absent.
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

/// One roster row, ids resolved. Level, class and zone are filled for **offline** members too:
/// the server keeps them in the guild table and puts them on the wire, so the pane greys the row
/// rather than blanking its columns — the opposite of the friend list, whose wire carries nothing
/// for an offline friend.
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
            // The wire sends no float at all for an online member, and the formatter reads
            // all-zeroes exactly as it reads nil.
            last_online: if online {
                Default::default()
            } else {
                last_online(m.last_online_days)
            },
        },
    }
}

/// The away tag a member's presence byte carries — `GetGuildRosterInfo`'s tenth return, and a
/// **string on every path**, `""` and never nil.
///
/// **DND is tested before AFK** (`0x4d13e4 test al,0x4` before `0x4d13fb test al,0x2`), so a
/// member flagged both displays as DND. The `online` flag beside it is the whole presence byte
/// against zero (`0x4d0c12 test dl,dl`), never a mask against the ONLINE bit — see
/// [`GuildRosterMember::is_online`].
pub(super) fn status_tag(presence: u8) -> &'static str {
    if presence & guild_presence::DND != 0 {
        CHAT_FLAG_DND
    } else if presence & guild_presence::AFK != 0 {
        CHAT_FLAG_AFK
    } else {
        ""
    }
}

/// The 1-based row a guid occupies in the shown order, `0` when it is not shown — the reference's
/// own guid→index conversion (`GetGuildRosterSelection 0x4d1890` → `0x4d1030`, a linear search
/// returning `index + 1`, so a member who left reads as "nothing selected").
pub(super) fn index_of(order: &[u64], guid: u64) -> u32 {
    if guid == 0 {
        return 0;
    }
    order
        .iter()
        .position(|g| *g == guid)
        .map_or(0, |i| i as u32 + 1)
}

/// Turn the Era API's guild intents into their sends.
///
/// Two of them need the app because only the app knows the display order: the note verbs carry a
/// 1-based **row** and the wire wants the member's **name**. Three are local-only — select, sort
/// and show-offline change what the next push says and send nothing at all.
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
                // The reference throttles this binding itself, to one request per 10 s against
                // `0xb73130` (`0x4d10d0`), and gives no signal when it swallows one. Real time,
                // not virtual: this is a span measured against the server, and the virtual clock's
                // 250 ms `max_delta` falls behind under a hitch (decision 0846).
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
                // Deliberately no local write: the reference's `SetGuildInfoText 0x4d2380` sends
                // and does NOT touch `0xb72720`, so the visible text only changes when the next
                // roster brings it back.
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
                // The client enforces the ladder's bounds itself, and silently: the reference's
                // Add button disables at ten ranks and its Remove button only appears above five
                // (`FriendsFrame.lua:887`, `:908`). Enforcing it here as well is what keeps a
                // macro or an addon from sending what the server would only ignore.
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

/// A note verb: resolve the display row to the member's name, write the note locally, and send
/// **only if the text actually changed** — the reference writes the record first and gates the
/// packet on that write having changed anything (`0x4d15e0`, whose `0x64a480` result is the
/// condition on the send).
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

/// A rank name the wire will accept, or `None`.
///
/// Over-long is not a polite refusal on this packet: `HandleGuildRankOpcode` /
/// `HandleGuildAddRankOpcode` run `ProcessAnticheatAction(… CHEAT_ACTION_KICK)`
/// (`Handlers/GuildHandler.cpp:580-584`, `:600-604`). The reference caps at the edit box's
/// `maxLetters`; we cap at the send too, and **drop rather than truncate** — a silently shortened
/// rank name is the wrong name, which is its own bug.
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

    /// DND is tested **before** AFK, so a member flagged both shows as DND — and the tag is a
    /// string on every path, including an offline member's.
    #[test]
    fn dnd_wins_over_afk_and_the_tag_is_always_a_string() {
        use guild_presence::{AFK, DND, OFFLINE, ONLINE};
        assert_eq!(status_tag(ONLINE | AFK | DND), "<DND>");
        assert_eq!(status_tag(ONLINE | DND), "<DND>");
        assert_eq!(status_tag(ONLINE | AFK), "<AFK>");
        assert_eq!(status_tag(ONLINE), "");
        assert_eq!(status_tag(OFFLINE), "", "never nil, even offline");
    }

    /// `online` is the whole presence byte against zero, not a mask against the ONLINE bit — so an
    /// unknown future bit reads as online, which is the safe direction (it is also what decides
    /// whether the wire carries the last-online float at all).
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

    /// An offline member keeps their level, class and zone — the guild table has them and the
    /// wire sends them, which is exactly what the friend list cannot do.
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

    /// An online member's last-online reads as all zeroes — the wire carries no float for them,
    /// and the formatter reads that exactly as it reads nil.
    #[test]
    fn an_online_row_reports_no_last_online() {
        let member = GuildRosterMember {
            presence: guild_presence::ONLINE,
            // Even if a value were somehow present, an online member never reaches the
            // arithmetic in the reference either (`0x4d14fd`).
            last_online_days: 9.0,
            ..Default::default()
        };
        assert_eq!(
            row(&member, None, None).info.last_online,
            Default::default()
        );
    }

    /// A row index maps through the *shown* order, and 0/past-the-end map to nothing.
    #[test]
    fn the_shown_order_maps_both_ways() {
        assert_eq!(index_of(&[11, 22, 33], 22), 2);
        assert_eq!(index_of(&[22, 11, 33], 22), 1, "same player, new row");
        assert_eq!(index_of(&[11, 33], 22), 0, "no longer listed");
        assert_eq!(index_of(&[11, 22], 0), 0, "nothing selected");
    }

    /// An over-long rank name is dropped, not truncated and not sent — the server kicks over it.
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
