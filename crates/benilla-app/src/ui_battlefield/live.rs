//! The battleground getters' reads at the call, the call host's half of
//! [`benilla_ui::script::BattlefieldSlotQuery`] and its kin: the queue slots, the list, the board
//! and the positions as the handlers left them, each answer derived as the reference's getter
//! derives it at the call (`0x4ab4a0`, `0x4ab0b0`, `0x4ab9d0`, `0x4abf90`).

use std::time::Instant;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_assets::MapCatalogRes;
use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    BattlefieldFlagQuery, BattlefieldFlagView, BattlefieldIconScaleQuery, BattlefieldListQuery,
    BattlefieldPositionCountQuery, BattlefieldPositionQuery, BattlefieldPositionView,
    BattlefieldScoreRowQuery, BattlefieldScoreSummaryQuery, BattlefieldSlotQuery,
    BattlefieldStatColumnQuery, BattlefieldTimes, BattlefieldTimesQuery,
};

use super::{list_view, slot_view, Battlefield};
use crate::names::NameCache;
use crate::net::{GuidIndex, SelfGuid};
use crate::ui_battlefield_positions::{flag_token, live_or_packet, BattlefieldPositions};
use crate::ui_battlefield_score::{BattlefieldScoreboard, BoardView};
use crate::ui_dialog_verbs::BattlefieldQueue;
use crate::ui_party::GroupState;
use crate::ui_world_map::{project_on_displayed, WorldMapUiData};

/// What the battleground getters read as they run; each input absent in a harness without it,
/// where the getter reads the VM's copy.
#[derive(SystemParam)]
pub(crate) struct BattlefieldLive<'w, 's> {
    queue: Option<Res<'w, BattlefieldQueue>>,
    list: Option<Res<'w, Battlefield>>,
    board: Option<Res<'w, BattlefieldScoreboard>>,
    positions: Option<Res<'w, BattlefieldPositions>>,
    maps: Option<Res<'w, MapCatalogRes>>,
    map_data: Option<Res<'w, WorldMapUiData>>,
    names: Option<Res<'w, NameCache>>,
    tables: crate::ui_unit::NameTables<'w>,
    group: Option<Res<'w, GroupState>>,
    me: Option<Res<'w, SelfGuid>>,
    guids: Option<Res<'w, GuidIndex>>,
    objects: crate::net::Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    /// Each unit's position as of here, composed through its parents on demand.
    transforms: bevy::transform::helper::TransformHelper<'w, 's>,
}

impl BattlefieldLive<'_, '_> {
    /// The player's side by race (`0x5efe00`: 0 Horde, 1 Alliance), -1 with no player or table.
    fn side(&self) -> i8 {
        self.objects
            .player()
            .and_then(|e| self.stores.get(e).ok())
            .and_then(|s| s.0.unit_race())
            .map_or(-1, |race| self.tables.get().pvp_team(race))
    }

    fn board_view(&self) -> Option<(&BattlefieldScoreboard, BoardView<'_>)> {
        Some((
            self.board.as_deref()?,
            BoardView {
                names: self.names.as_deref()?,
                tables: self.tables.get(),
            },
        ))
    }

    /// The active slot's map, 0 with none (`[0x8457cc]`).
    fn active_map(&self) -> u32 {
        self.queue
            .as_deref()
            .and_then(BattlefieldQueue::active_map)
            .unwrap_or(0)
    }

    /// The teammates the getters walk: not us, no party slot, not on the raid roster
    /// (`0x4ac00b`, `0x4ac017`), in wire order.
    fn teammates(&self) -> impl Iterator<Item = &benilla_protocol::messages::BattlefieldPosition> {
        let me = self.me.as_deref().and_then(|m| m.0);
        let group = self.group.as_deref();
        self.positions
            .as_deref()
            .map_or(&[][..], BattlefieldPositions::teammates)
            .iter()
            .filter(move |p| {
                me != Some(p.guid) && !group.is_some_and(|g| g.party_or_raid_member(p.guid))
            })
    }

    /// `(x, y)` under the active slot's map, onto the world map's `selection`, `(0, 0)` off it
    /// (`0x4a7360`).
    fn project(&self, selection: (u32, u32, Option<u32>), (x, y): (f32, f32)) -> (f32, f32) {
        let map = self.active_map();
        self.map_data
            .as_deref()
            .and_then(|d| project_on_displayed(d, selection, map, x, y))
            .unwrap_or((0.0, 0.0))
    }

    fn position(&self, q: &BattlefieldPositionQuery) -> Option<Option<BattlefieldPositionView>> {
        self.positions.as_ref()?;
        let index = usize::try_from(q.index).ok().and_then(|i| i.checked_sub(1));
        let Some(p) = index.and_then(|i| self.teammates().nth(i)) else {
            return Some(None);
        };
        let at = live_or_packet(p, self.guids.as_deref(), &self.transforms);
        let name = self
            .names
            .as_deref()
            .and_then(|n| n.resolve_player(p.guid, crate::query_cache::Ask::Read))
            .map(str::to_string);
        Some(Some(BattlefieldPositionView {
            uv: self.project(q.selection, at),
            name,
            guid: p.guid,
        }))
    }

    fn flag(&self, q: &BattlefieldFlagQuery) -> Option<Option<BattlefieldFlagView>> {
        let positions = self.positions.as_deref()?;
        Some(positions.carrier().filter(|c| c.guid != 0).map(|c| {
            let at = live_or_packet(c, self.guids.as_deref(), &self.transforms);
            BattlefieldFlagView {
                uv: self.project(q.selection, at),
                token: flag_token(self.side()).map(str::to_string),
            }
        }))
    }
}

impl crate::script_calls::live::LiveFamily for BattlefieldLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        let catalog = self.maps.as_deref().map(|m| &m.0);
        ask.answer(|q: &BattlefieldSlotQuery| {
            let queue = self.queue.as_deref()?;
            Some(slot_view(
                queue.slots().get(q.0)?.as_ref(),
                catalog,
                Instant::now(),
            ))
        }) || ask.answer(|_: &BattlefieldTimesQuery| {
            let queue = self.queue.as_deref()?;
            let now = Instant::now();
            Some(BattlefieldTimes {
                instance_expiration_ms: queue.instance_expiration_ms(now),
                run_time_ms: queue.run_time_ms(now),
            })
        }) || ask.answer(|_: &BattlefieldListQuery| {
            Some(list_view(self.list.as_deref()?, catalog, self.side()))
        }) || ask.answer(|_: &BattlefieldScoreSummaryQuery| Some(self.board.as_deref()?.summary()))
            || ask.answer(|q: &BattlefieldScoreRowQuery| {
                let (board, at) = self.board_view()?;
                Some(board.row(q.0, at))
            })
            || ask.answer(|q: &BattlefieldStatColumnQuery| Some(self.board.as_deref()?.column(q.0)))
            || ask.answer(|_: &BattlefieldPositionCountQuery| {
                self.positions.as_ref()?;
                Some(self.teammates().count())
            })
            || ask.answer(|q: &BattlefieldPositionQuery| self.position(q))
            || ask.answer(|q: &BattlefieldFlagQuery| self.flag(q))
            || ask.answer(|_: &BattlefieldIconScaleQuery| {
                // The active slot's row, map 0's with no slot, 1.0 for a missing row.
                Some(
                    catalog
                        .and_then(|m| m.battleground(self.active_map()))
                        .map_or(1.0, |b| b.minimap_icon_scale),
                )
            })
    }
}
