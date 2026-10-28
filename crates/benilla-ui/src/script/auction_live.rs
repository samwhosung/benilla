//! The auction getters' live reads: the questions they ask the lent host
//! ([`super::live::ask`]) and the VM's half of each answer. The VM holds each list's order, as its
//! sorts run where the reference's do; the house holds the records and the throttle, and the
//! caches the fields, all read at the call as the reference's getters read its arrays
//! (`0x4cee40`, `0x4cf2f0`, `0x4cf470`, `0x4cfbb0`, `0x4ce170`).

use mlua::Lua;

use super::auction::{list_index_of, AuctionItemRow};
use super::Model;

/// A row's record as the house holds it at the call, resolved off the item and name caches as the
/// getters read them (`0x4cef96`, `0x4cf144`, `0x4cf1b2`): the auction `auction_id` of `list`, on
/// the page `page` the VM's row came from. The host declines a page it no longer holds, and the
/// getter reads the VM's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuctionRowQuery {
    pub list: usize,
    pub page: u32,
    pub auction_id: u32,
}

impl super::live::LiveQuery for AuctionRowQuery {
    type Answer = AuctionItemRow;
}

/// `CanSendAuctionQuery`'s throttle at the call (`0x4cfbb0`): no throttle armed, or the tick past
/// it (`[0xb72638]`, `0x4cfbc1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuctionCanQuery;

impl super::live::LiveQuery for AuctionCanQuery {
    type Answer = bool;
}

/// The deposit rate of the house the session holds (`[0xb72600]`), its `AuctionHouse.dbc` row read
/// at the call (`0x4ce179`-`0x4ce18e`), 0 with no row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuctionDepositRate;

impl super::live::LiveQuery for AuctionDepositRate {
    type Answer = u32;
}

/// The VM's row at the 1-based `index` of `list`, and the page it came from.
fn held_row(lua: &Lua, list: usize, index: usize) -> Option<(u32, AuctionItemRow)> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let l = &model.auction.as_ref()?.lists[list];
    let row = index.checked_sub(1).and_then(|n| l.rows.get(n))?.clone();
    Some((l.page, row))
}

/// The record at the VM's row read live ([`AuctionRowQuery`]): the VM holds the order, the house
/// the record and the caches its fields, as the reference's getters read the array and the caches
/// at the call. With no host lent, or a page the house no longer holds, the VM's row.
fn live_row(lua: &Lua, page: u32, list: usize, held: AuctionItemRow) -> AuctionItemRow {
    let query = AuctionRowQuery {
        list,
        page,
        auction_id: held.auction_id,
    };
    super::live::ask(lua, &query).unwrap_or(held)
}

/// The row at the 1-based `index` of the list `kind` names, read live ([`live_row`]).
pub(super) fn row_at(lua: &Lua, kind: &str, index: usize) -> Option<AuctionItemRow> {
    live_row_at(lua, list_index_of(kind)?, index)
}

/// [`row_at`] by list index.
pub(super) fn live_row_at(lua: &Lua, list: usize, index: usize) -> Option<AuctionItemRow> {
    let (page, held) = held_row(lua, list, index)?;
    Some(live_row(lua, page, list, held))
}

/// Every row of `list` read live, in the VM's order, ahead of a sort the call runs: the comparator
/// reads the records and the caches as they stand (`0x4cdbda`).
pub(super) fn refresh_live(lua: &Lua, list: usize) {
    let (page, rows) = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let Some(l) = model.auction.as_ref().map(|a| &a.lists[list]) else {
            return;
        };
        (l.page, l.rows.clone())
    };
    let rows: Vec<AuctionItemRow> = rows
        .into_iter()
        .map(|r| live_row(lua, page, list, r))
        .collect();
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Some(l) = model.auction.as_mut().map(|a| &mut a.lists[list]) {
        if l.page == page && l.rows.len() == rows.len() {
            l.rows = rows;
        }
    }
}

/// The open house's deposit rate, read at the call ([`AuctionDepositRate`]); with no host lent, the
/// VM's copy.
pub(super) fn deposit_rate(lua: &Lua) -> u32 {
    super::live::ask(lua, &AuctionDepositRate).unwrap_or_else(|| {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        model.auction.as_ref().map_or(0, |a| a.deposit_percent)
    })
}
