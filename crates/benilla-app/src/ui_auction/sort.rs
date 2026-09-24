//! The auction list sort (decision 1511 §6) — client-side, because the wire carries no sort field
//! at all: `CMSG_AUCTION_LIST_ITEMS` is ten fields and none of them is a column.
//!
//! **It is not an asc → desc → none cycle**, which is the thing everyone assumes and gets wrong.
//! Each list owns a *most-recently-clicked stack* of up to eight `(key, reversed)` pairs. Clicking
//! the column that is already primary toggles its direction; clicking any other column promotes it
//! to primary **keeping the direction it already remembers**. Two consequences fall out, and both
//! are visible in the window: a column that is not currently sorting anything can still draw a
//! flipped arrow, and the sort is *stable across keys* — dropping "quality" to second place still
//! breaks ties by it.
//!
//! `0x4cd940` *is* the auction comparator, the sort *is* an 8-key MRU cascade, and each list owns
//! its own `{col,dir}` stack. The per-key directions below are the reference's, not our guess —
//! three of them were wrong when this file first landed, and all three were ones a shopping UI
//! makes the "obvious" way round.

use std::cmp::Ordering;

use super::AuctionRow;

// The eight keys live in `benilla_ui::script::SORT_KEYS` — the Lua binding is what refuses an
// unknown one, with the reference's own `Usage:` error, so this side never re-lists them.

/// The reference keeps eight slots per list. Past that the oldest key is simply forgotten, which
/// is invisible in a window with at most six columns — but the bound is the reference's, so it is
/// the bound here.
const DEPTH: usize = 8;

/// One list's sort state: the most-recently-clicked key first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SortStack {
    entries: Vec<(String, bool)>,
}

impl SortStack {
    /// A header click. Already primary → flip its direction. Anywhere else (or brand new) → it
    /// becomes primary, **carrying the direction it had**, and everything above it shifts down.
    pub(crate) fn click(&mut self, key: &str) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k == key) {
            if pos == 0 {
                self.entries[0].1 = !self.entries[0].1;
            } else {
                let entry = self.entries.remove(pos);
                self.entries.insert(0, entry);
            }
        } else {
            self.entries.insert(0, (key.to_string(), false));
            self.entries.truncate(DEPTH);
        }
    }

    /// The stack as the engine pushes it to Lua (`IsAuctionSortReversed` reads it back).
    pub(crate) fn pairs(&self) -> Vec<(String, bool)> {
        self.entries.clone()
    }

    /// Order `rows` by the whole stack: primary key first, each remaining key breaking the ties
    /// above it. An empty stack leaves the server's own order alone — which for a browse page is
    /// buyout ascending, and is a perfectly good default.
    pub(crate) fn apply(&self, rows: &mut [AuctionRow]) {
        if self.entries.is_empty() {
            return;
        }
        // `sort_by` is stable, so equal-under-every-key rows keep the order the server sent —
        // which keeps a repaint from shuffling rows the player is looking at.
        rows.sort_by(|a, b| {
            for (key, reversed) in &self.entries {
                let ord = compare_by(key, a, b);
                let ord = if *reversed { ord.reverse() } else { ord };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });
    }
}

/// One key's comparison, in the direction a *first* click on that column produces (`0x4cd940`'s
/// arms, read as "which row appears first").
///
/// Three of these run the opposite way to the intuition, and each is intuitive once you know what
/// the column is for: **quality** opens with the epics, **buyout** opens with the most expensive
/// item rather than the cheapest, and **status** opens with the auctions you are already in. Only
/// level, bid, duration and the two name columns run the "small/soon/A-first" way round.
fn compare_by(key: &str, a: &AuctionRow, b: &AuctionRow) -> Ordering {
    match key {
        "level" => a.level.cmp(&b.level),
        // Highest quality first — the epics, not the greys.
        "quality" => b.quality.cmp(&a.quality),
        // What the row SHOWS as the current bid: the opening price until somebody has bid.
        "bid" => a.displayed_bid().cmp(&b.displayed_bid()),
        // The bucket, not the raw milliseconds — two rows in the same bucket read as equal to the
        // player, so ordering them by a millisecond difference they cannot see would make the sort
        // look unstable.
        "duration" => a.time_left.cmp(&b.time_left),
        // Highest buyout first — which also settles what a *missing* buyout does without a
        // special case: `0` is "no buyout", and descending drops it to the bottom on its own,
        // exactly where it belongs. The ascending version this file shipped with needed a
        // hand-written arm to avoid opening the column with every buyout-less auction.
        "buyout" => b.buyout.cmp(&a.buyout),
        // The auctions you are already in, first — the Bid tab's High Bidder / Outbid column.
        "status" => b.high_bidder.cmp(&a.high_bidder),
        "name" => name_of(a).cmp(name_of(b)),
        "seller" => owner_of(a).cmp(owner_of(b)),
        _ => Ordering::Equal,
    }
}

/// A row whose template has not landed yet sorts as the empty string rather than jumping position
/// when the name arrives — the answer is at least stable while the async fill happens.
fn name_of(r: &AuctionRow) -> &str {
    r.name.as_deref().unwrap_or("")
}

fn owner_of(r: &AuctionRow) -> &str {
    r.owner.as_deref().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(level: u32, bid: u32, buyout: u32, name: &str) -> AuctionRow {
        AuctionRow {
            level,
            start_bid: bid,
            buyout,
            name: Some(name.to_string()),
            ..Default::default()
        }
    }

    /// The promote-and-toggle law, which is the whole reason this is a stack and not a field.
    #[test]
    fn a_click_promotes_but_only_the_primary_toggles() {
        let mut s = SortStack::default();

        s.click("bid");
        assert_eq!(s.pairs(), vec![("bid".into(), false)]);

        // Re-clicking the primary flips it.
        s.click("bid");
        assert_eq!(s.pairs(), vec![("bid".into(), true)]);

        // A new key takes the front; the old primary keeps BOTH its place behind it and its
        // reversed flag.
        s.click("quality");
        assert_eq!(
            s.pairs(),
            vec![("quality".into(), false), ("bid".into(), true)]
        );

        // Promoting a remembered key carries its direction back to the front unflipped — this is
        // the case a plain asc/desc cycle gets wrong.
        s.click("bid");
        assert_eq!(
            s.pairs(),
            vec![("bid".into(), true), ("quality".into(), false)],
            "promotion is not a toggle"
        );
        let rev = |key: &str| s.pairs().iter().any(|(k, r)| k == key && *r);
        assert!(rev("bid"));
        assert!(!rev("quality"), "remembered, never reversed");
        assert!(!rev("seller"), "never clicked at all");
    }

    /// Eight slots, oldest forgotten — the reference's own bound.
    #[test]
    fn the_stack_is_eight_deep() {
        let mut s = SortStack::default();
        for k in benilla_ui::script::SORT_KEYS {
            s.click(k);
        }
        assert_eq!(s.pairs().len(), 8);
        // A ninth distinct key would push one out; every key we have IS one of the eight, so
        // re-clicking only reorders.
        s.click("level");
        assert_eq!(s.pairs().len(), 8, "re-click reorders, never grows");
        assert_eq!(s.pairs()[0].0, "level");
    }

    /// Keys below the primary break its ties — the reason the whole stack is applied and not just
    /// its head.
    #[test]
    fn lower_keys_break_the_primary_s_ties() {
        let mut rows = vec![
            row(10, 500, 0, "Bravo"),
            row(10, 100, 0, "Alpha"),
            row(5, 900, 0, "Charlie"),
        ];
        let mut s = SortStack::default();
        s.click("bid"); // secondary
        s.click("level"); // primary
        s.apply(&mut rows);
        let got: Vec<_> = rows.iter().map(|r| r.name.clone().unwrap()).collect();
        assert_eq!(got, vec!["Charlie", "Alpha", "Bravo"], "level, then bid");
    }

    /// Buyout opens HIGHEST-first (`0x4cdaf2`) — and a buyout of `0` means "none", not
    /// "free", so descending puts it last with no special case.
    #[test]
    fn buyout_opens_highest_first_and_no_buyout_sinks() {
        let mut rows = vec![
            row(1, 0, 0, "NoBuyout"),
            row(1, 0, 100, "Cheap"),
            row(1, 0, 5000, "Pricey"),
        ];
        let mut s = SortStack::default();
        s.click("buyout");
        s.apply(&mut rows);
        let got: Vec<_> = rows.iter().map(|r| r.name.clone().unwrap()).collect();
        assert_eq!(got, vec!["Pricey", "Cheap", "NoBuyout"]);
    }

    /// The three columns that run opposite to the intuition, pinned together so a future "fix"
    /// has to argue with the comparator `0x4cd940` rather than with a hunch.
    #[test]
    fn quality_and_status_open_highest_and_mine_first() {
        let mut rows = vec![
            AuctionRow {
                quality: Some(1),
                high_bidder: false,
                name: Some("Common".into()),
                ..Default::default()
            },
            AuctionRow {
                quality: Some(4),
                high_bidder: true,
                name: Some("Epic".into()),
                ..Default::default()
            },
        ];
        let mut s = SortStack::default();
        s.click("quality");
        s.apply(&mut rows);
        assert_eq!(rows[0].name.as_deref(), Some("Epic"), "epics first");

        let mut s = SortStack::default();
        s.click("status");
        s.apply(&mut rows);
        assert_eq!(
            rows[0].name.as_deref(),
            Some("Epic"),
            "the auction you are already bidding in, first"
        );
    }

    /// No sort clicked = the server's own order, untouched.
    #[test]
    fn an_empty_stack_leaves_the_servers_order_alone() {
        let mut rows = vec![row(9, 0, 0, "Zulu"), row(1, 0, 0, "Alpha")];
        SortStack::default().apply(&mut rows);
        let got: Vec<_> = rows.iter().map(|r| r.name.clone().unwrap()).collect();
        assert_eq!(got, vec!["Zulu", "Alpha"]);
    }
}
