//! The auction list sort, client-side since `CMSG_AUCTION_LIST_ITEMS` carries no sort field: each
//! list keeps the reference's most-recently-clicked stack of `(key, reversed)` pairs (`0x4cd940`),
//! and every key below the primary breaks ties.

use std::cmp::Ordering;

use super::AuctionRow;

// The keys are `benilla_ui::script::SORT_KEYS`; the Lua binding refuses any other.

/// The reference's eight slots per list; past that the oldest key is forgotten.
const DEPTH: usize = 8;

/// One list's sort state: the most-recently-clicked key first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SortStack {
    entries: Vec<(String, bool)>,
}

impl SortStack {
    /// A header click: the primary flips; any other key becomes primary with the direction it had.
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

    /// The stack as pushed to Lua, where `IsAuctionSortReversed` reads it.
    pub(crate) fn pairs(&self) -> Vec<(String, bool)> {
        self.entries.clone()
    }

    /// Order `rows` by the whole stack, primary key first; an empty stack keeps the server's order.
    pub(crate) fn apply(&self, rows: &mut [AuctionRow]) {
        if self.entries.is_empty() {
            return;
        }
        // A stable sort: rows equal under every key keep the server's order across repaints.
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

/// One key's order on a first click, per `0x4cd940`'s arms: quality, buyout and status open
/// highest first; level, bid, duration, name and seller open lowest first.
fn compare_by(key: &str, a: &AuctionRow, b: &AuctionRow) -> Ordering {
    match key {
        "level" => a.level.cmp(&b.level),
        "quality" => b.quality.cmp(&a.quality),
        "bid" => a.displayed_bid().cmp(&b.displayed_bid()),
        // The bucket; the reference compares the exact deadline.
        "duration" => a.time_left.cmp(&b.time_left),
        // Highest first, which also sinks a 0 (no buyout) to the bottom.
        "buyout" => b.buyout.cmp(&a.buyout),
        // Our own bids first; the reference also ranks another bidder's above no bid.
        "status" => b.high_bidder.cmp(&a.high_bidder),
        "name" => name_of(a).cmp(name_of(b)),
        "seller" => owner_of(a).cmp(owner_of(b)),
        _ => Ordering::Equal,
    }
}

/// A row whose name has not landed sorts as the empty string.
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

    #[test]
    fn a_click_promotes_but_only_the_primary_toggles() {
        let mut s = SortStack::default();

        s.click("bid");
        assert_eq!(s.pairs(), vec![("bid".into(), false)]);

        // Re-clicking the primary flips it.
        s.click("bid");
        assert_eq!(s.pairs(), vec![("bid".into(), true)]);

        // A new key takes the front; the old primary keeps its place behind it and its flag.
        s.click("quality");
        assert_eq!(
            s.pairs(),
            vec![("quality".into(), false), ("bid".into(), true)]
        );

        // Promoting a remembered key keeps its direction.
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

    #[test]
    fn the_stack_is_eight_deep() {
        let mut s = SortStack::default();
        for k in benilla_ui::script::SORT_KEYS {
            s.click(k);
        }
        assert_eq!(s.pairs().len(), 8);
        // Every key is one of the eight, so a re-click only reorders.
        s.click("level");
        assert_eq!(s.pairs().len(), 8, "re-click reorders, never grows");
        assert_eq!(s.pairs()[0].0, "level");
    }

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

    /// The comparator's buyout arm is `0x4cdaf2`.
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

    /// Both open highest first in the reference's comparator (`0x4cd940`).
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

    #[test]
    fn an_empty_stack_leaves_the_servers_order_alone() {
        let mut rows = vec![row(9, 0, 0, "Zulu"), row(1, 0, 0, "Alpha")];
        SortStack::default().apply(&mut rows);
        let got: Vec<_> = rows.iter().map(|r| r.name.clone().unwrap()).collect();
        assert_eq!(got, vec!["Zulu", "Alpha"]);
    }
}
