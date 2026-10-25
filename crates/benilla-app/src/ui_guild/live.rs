//! The guild getters' questions at the call, the call host's half of
//! [`benilla_ui::script::GuildRead`]: the roster and the guild records as of the drain's place
//! ([`GuildState::seen`]), the active player's guild fields as of there too
//! ([`crate::ui_unit::views::ViewStores`]), each answer derived as the reference's getters
//! derive theirs (`GetGuildRosterInfo` `0x4d1200`, `GuildControlGetRankName` `0x4d1e90`,
//! `GetGuildInfo` `0x4c9330`): the order is the roster's sorted array (`0x4d0d50`), the rank
//! name comes from the guild's record, looked up with a callback at each call (`0x560e30`), and a
//! record not cached fails the call.

use std::sync::{Arc, Mutex, PoisonError};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::ObjectType;
use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    GuildAnswer, GuildMemberInfo, GuildRead, OwnGuild, UnitGuild, UnitGuildRead,
};

use super::sort::{RosterRow, SortStack};
use super::view::{GuildView, Roster};
use super::{feed, GuildLocal, GuildState, Identity, Masker};
use crate::area::AreaTableRes;
use crate::net::{NetCommands, ObjectStore};
use crate::query_cache::Ask;
use crate::ui_unit::{NameTables, UnitNames};

/// What a display order was built from: the member list, the sort chain and show-offline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OrderKey {
    members_rev: u64,
    sort: SortStack,
    show_offline: bool,
}

/// The last display order built, kept so the loop over a roster's rows sorts it once.
pub(super) type OrderCache = Mutex<Option<(OrderKey, Arc<Vec<RosterRow>>)>>;

impl GuildLocal {
    /// `roster` in display order, every member, sorted and never filtered: the array the
    /// reference sorts after each roster message and each sort change (`0x4d0d50`, `0x4d0d32`,
    /// `0x4d0f95`, `0x4d1018`), which is a function of the list, the chain and show-offline, so
    /// built again only when one moves. The rows carry no rank name: the guild's record names it
    /// where the row is read.
    pub(super) fn ordered(
        &self,
        roster: &Roster,
        show_offline: bool,
        areas: Option<&AreaTableCatalog>,
        names: UnitNames<'_>,
    ) -> Arc<Vec<RosterRow>> {
        let key = OrderKey {
            members_rev: roster.members_rev,
            sort: self.sort,
            show_offline,
        };
        let mut slot = self.order.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((built, rows)) = slot.as_ref() {
            if *built == key {
                return rows.clone();
            }
        }
        let mut rows: Vec<RosterRow> = roster
            .members
            .iter()
            .map(|m| feed::row(m, areas, None, names))
            .collect();
        self.sort.order(&mut rows, show_offline);
        let rows = Arc::new(rows);
        *slot = Some((key, rows.clone()));
        rows
    }
}

/// The row at the 1-based `index`, `None` past the member list or below the first (`0x4d12b9`).
pub(super) fn row_at(rows: &[RosterRow], index: i64) -> Option<&RosterRow> {
    usize::try_from(index)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| rows.get(i))
}

/// `row`'s info with its rank named by `identity`, the guild's record (`record + 0x64 + 0x40 *
/// rank`, `0x4d12e2`), masked as the getter masks it (`0x4d12fd`, `dl = 1`).
fn named(row: &RosterRow, identity: Option<&Identity>, masker: &Masker) -> GuildMemberInfo {
    let mut info = row.info.clone();
    info.rank = identity
        .map(|i| i.rank_name(info.rank_index).to_string())
        .unwrap_or_default();
    masker.cached(&mut info.rank);
    info
}

/// What the guild getters read as they run.
#[derive(SystemParam)]
pub(crate) struct GuildLive<'w, 's> {
    guild: Option<Res<'w, GuildState>>,
    local: Option<Res<'w, GuildLocal>>,
    areas: Option<Res<'w, AreaTableRes>>,
    tables: NameTables<'w>,
    commands: Option<Res<'w, NetCommands>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    masker: Masker<'w>,
}

impl GuildLive<'_, '_> {
    /// The active player's store as of here (`ClntObjMgrGetActivePlayer`, then the object).
    fn player(&self) -> Option<&ObjectStore> {
        self.objects.player().and_then(|e| self.stores.get(e).ok())
    }

    /// The guild's record under `guild_id`, looked up with a callback as `0x560e30` does at each
    /// call (it asks on a miss, once while pending, and a refused guild again), read as of here: a
    /// record that landed in a later packet is not yet there.
    fn record(&self, guild: &GuildState, view: &GuildView, guild_id: u32) -> Option<Arc<Identity>> {
        match self.commands.as_deref() {
            // The lookup of `0x4d1291` and `0x4d1f29`, with the callback `0x4d1480` and no dedupe.
            Some(commands) => guild.wait_for_roster(guild_id, commands),
            None => {
                guild.resolve_identity(guild_id, Ask::Read);
            }
        }
        view.identities.get(&guild_id).cloned()
    }

    /// `read`'s answer as of here; `None` in a harness without the guild state, where the
    /// snapshot answers.
    pub(crate) fn read(&self, read: GuildRead) -> Option<GuildAnswer> {
        let (guild, local) = (self.guild.as_deref()?, self.local.as_deref()?);
        let view = guild.seen();
        let roster = &view.roster;
        let ordered = |show_offline: bool| {
            local.ordered(
                roster,
                show_offline,
                self.areas.as_deref().map(|a| &a.0),
                self.tables.get(),
            )
        };
        Some(match read {
            GuildRead::Own => GuildAnswer::Own(self.player().map(|store| {
                let (id, rank) = (store.0.player_guild_id(), store.0.player_guild_rank());
                OwnGuild {
                    in_guild: id != 0,
                    rank_index: rank,
                    rights: roster.rights(rank),
                }
            })),
            GuildRead::Texts => GuildAnswer::Texts {
                motd: roster.motd.clone(),
                info: roster.info_text.clone(),
            },
            // `0x4d11ab`-`0x4d11bd`: the whole count with the flag set or the argument true.
            GuildRead::Count { all, show_offline } => GuildAnswer::Count(if all || show_offline {
                roster.members.len()
            } else {
                roster.online
            }),
            GuildRead::Member {
                index,
                show_offline,
                named: wants_record,
            } => GuildAnswer::Member(self.member(
                guild,
                view,
                &ordered,
                index,
                show_offline,
                wants_record,
            )),
            GuildRead::GuidAt {
                index,
                show_offline,
            } => GuildAnswer::Guid(row_at(&ordered(show_offline), index).map(|r| r.guid)),
            GuildRead::RowOf { guid, show_offline } => {
                GuildAnswer::Row(ordered(show_offline).iter().position(|r| r.guid == guid))
            }
            GuildRead::RankCount => GuildAnswer::Count(roster.rank_rights.len()),
            GuildRead::RankName(rank) => GuildAnswer::RankName(self.rank_name(guild, view, rank)),
            GuildRead::Rights(rank) => GuildAnswer::Rights(roster.rights(rank)),
            GuildRead::Unit(guid) => GuildAnswer::Unit(self.unit(view, guid)),
        })
    }

    /// `GetGuildRosterInfo`'s and `GetGuildRosterLastOnline`'s row (`0x4d1200`, `0x4d14a0`): with
    /// no player the call fails before anything is asked (`0x4d125f`), then the guild's record is
    /// looked up from the player's guild id (`0x4d1291`) where the getter names the rank, and a
    /// miss fails it, then the index is held to the whole roster (`0x4d12bb`).
    fn member(
        &self,
        guild: &GuildState,
        view: &GuildView,
        ordered: &dyn Fn(bool) -> Arc<Vec<RosterRow>>,
        index: i64,
        show_offline: bool,
        wants_record: bool,
    ) -> Option<GuildMemberInfo> {
        // `GetGuildRosterLastOnline` has no player gate (`0x4d14a0`): the call fails with no
        // player only where it names the rank (`0x4d125f`).
        let identity = if wants_record {
            let store = self.player()?;
            Some(self.record(guild, view, store.0.player_guild_id())?)
        } else {
            None
        };
        let rows = ordered(show_offline);
        Some(named(
            row_at(&rows, index)?,
            identity.as_deref(),
            &self.masker,
        ))
    }

    /// `GuildControlGetRankName`'s name (`0x4d1e90`): nil with no player or past the rank count
    /// (`0x4d1f0a`, `0x4d1f12`) before the record is looked up (`0x4d1f3e`), and nil on a miss.
    fn rank_name(&self, guild: &GuildState, view: &GuildView, rank: u32) -> Option<String> {
        let store = self.player()?;
        if rank as usize >= view.roster.rank_rights.len() {
            return None;
        }
        let identity = self.record(guild, view, store.0.player_guild_id())?;
        let mut name = identity.rank_name(rank).to_string();
        self.masker.cached(&mut name);
        Some(name)
    }

    /// `GetGuildInfo`'s unit (`0x4c9330`): a held object of the player type (`0x4c9397`) with a
    /// guild id (`0x4c93ac`); its record is looked up by the getter's own ask, which leaves the
    /// unit's waiter (`0x4c93d2`).
    fn unit(&self, view: &GuildView, guid: u64) -> UnitGuildRead {
        let store = self
            .stores
            .at(guid, self.objects.entity(guid))
            .filter(|s| s.0.created_as() == Some(ObjectType::Player));
        let Some(store) = store else {
            return UnitGuildRead::None;
        };
        let (guild_id, rank_index) = (store.0.player_guild_id(), store.0.player_guild_rank());
        if guild_id == 0 {
            return UnitGuildRead::None;
        }
        match view.identities.get(&guild_id) {
            Some(identity) => {
                // The rank's name is masked as the call returns it (`0x4c9402`, `dl = 1`).
                let mut rank_name = identity.rank_name(rank_index).to_string();
                self.masker.cached(&mut rank_name);
                UnitGuildRead::Named(UnitGuild {
                    name: identity.name.clone(),
                    rank_name,
                    rank_index,
                })
            }
            None => UnitGuildRead::Miss { guild_id },
        }
    }
}

impl crate::script_calls::live::LiveFamily for GuildLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &GuildRead| self.read(*q))
    }
}
