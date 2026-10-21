//! The auction lists' one sort: each list's eight-slot key stack, the comparator `0x4cd940` and the
//! CRT `qsort` the three sort tails run (`0x4cd8b0`, `0x4cdcd0`, `0x4cddc0`). `CMSG_AUCTION_LIST_ITEMS`
//! carries no sort, so the order is the client's own; the VM's auction bindings sort with this, and
//! the app keeps its copy of the stacks with it.

use crate::script::{AuctionItemRow, BIDDER, LIST, OWNER, SORT_KEYS};

/// The column a sort name names, matched without case as `SStrCmpI` (`0x64a4c0`) matches it;
/// `None` for any other name, which sorts nothing (`0x4cfd90`).
pub fn sort_column(name: &str) -> Option<u8> {
    SORT_KEYS
        .iter()
        .position(|k| crate::crt::str_cmp_i(k, name) == 0)
        .map(|i| i as u8)
}

/// The Bids and Auctions lists' starting columns, the twin tables `0x849084` and `0x849064`:
/// status, duration, bid, level, name, quality, buyout, seller.
const OWNED_SEED: [u8; 8] = [5, 3, 2, 0, 6, 1, 4, 7];

/// One list's sort key stack, eight `{column, reversed}` slots with the primary first (browse
/// `0xb725b0`, owner `0xb72420`, bidder `0xb72548`). It holds every column from the world entry on,
/// so every sort walks all eight keys and a list is always fully ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuctionSortStack([(u8, bool); 8]);

impl AuctionSortStack {
    /// The stack the world entry gives `list` (`0x4cc0f0`, `0x4cc236`-`0x4cc274`): Browse in column
    /// order (`0x4cc246`), Bids and Auctions from their tables, every key unreversed.
    pub fn seeded(list: usize) -> Self {
        let columns = if list == BIDDER || list == OWNER {
            OWNED_SEED
        } else {
            std::array::from_fn(|i| i as u8)
        };
        Self(columns.map(|c| (c, false)))
    }

    /// The three lists' stacks at world entry, indexed as the lists are.
    pub fn seeded_all() -> [Self; 3] {
        std::array::from_fn(Self::seeded)
    }

    /// A header click (`0x4cd850`, `0x4cdc70`, `0x4cdd60`): the primary flips (`0x4cd876 sete`);
    /// any other column moves to the front with the flag it had, the slots above it shifting down
    /// (`0x4cd887`-`0x4cd895`); a column the stack does not hold changes nothing (`0x4cd861`).
    pub fn promote(&mut self, column: u8) {
        let Some(at) = self.0.iter().position(|&(c, _)| c == column) else {
            return;
        };
        let (_, reversed) = self.0[at];
        let flag = if at == 0 { !reversed } else { reversed };
        self.0.copy_within(0..at, 1);
        self.0[0] = (column, flag);
    }

    /// The column's reversed flag, `None` for a column the stack does not hold.
    pub fn reversed(&self, column: u8) -> Option<bool> {
        self.0.iter().find(|&&(c, _)| c == column).map(|&(_, r)| r)
    }

    /// The slots, primary first.
    pub fn slots(&self) -> [(u8, bool); 8] {
        self.0
    }

    /// The sort adapters (`0x4cd8e0`, `0x4cdd00`, `0x4cddf0`): each key in stack order until one
    /// differs, negated when its slot is reversed (`0x4cd926`-`0x4cd92f`); 0 when all eight tie.
    fn compare(&self, a: &AuctionItemRow, b: &AuctionItemRow) -> i32 {
        for &(column, reversed) in &self.0 {
            let ord = compare(column, a, b);
            if ord != 0 {
                return if reversed { -ord } else { ord };
            }
        }
        0
    }

    /// A sort tail's `qsort` (`0x4cd8c2`): `rows` reordered by the whole stack, ties and rows the
    /// comparator cannot order left where the CRT leaves them.
    pub(crate) fn sort(&self, rows: &mut [AuctionItemRow]) {
        crate::crt::qsort(rows, |a, b| self.compare(a, b));
    }
}

impl Default for AuctionSortStack {
    /// The Browse list's stack.
    fn default() -> Self {
        Self::seeded(LIST)
    }
}

/// The comparator `0x4cd940`: `a` against `b` by one column, unreversed, below 0 when `a` goes first.
/// Level, quality and name read the item records and tie when either is not cached
/// (`0x4cd9b3`, `0x4cda1f`, `0x4cda90`); the other five read the auction records alone.
fn compare(column: u8, a: &AuctionItemRow, b: &AuctionItemRow) -> i32 {
    // `+1` when `a` sorts after `b`, `-1` before, 0 on a tie.
    let after = |tie: bool, a_after: bool| match (tie, a_after) {
        (true, _) => 0,
        (false, true) => 1,
        (false, false) => -1,
    };
    let cached = a.quality.is_some() && b.quality.is_some();
    match column {
        // Level, the lowest first, both floored at 1 (`0x4cda98`-`0x4cdac3`, `setge`).
        0 if cached => {
            let (la, lb) = (floor_level(a), floor_level(b));
            after(la == lb, la >= lb)
        }
        // Quality, the highest first (`0x4cda2d`-`0x4cda42`, `setl`).
        1 if cached => {
            let (qa, qb) = (a.quality.unwrap_or(0) as i32, b.quality.unwrap_or(0) as i32);
            after(qa == qb, qa < qb)
        }
        // The bid shown, the current bid or the opening price without one, the lowest first,
        // compared signed (`0x4cdb81`-`0x4cdbac`, `setge`).
        2 => {
            let (ba, bb) = (shown_bid(a), shown_bid(b));
            after(ba == bb, ba >= bb)
        }
        // The deadline, the soonest first, compared unsigned (`0x4cdad4`-`0x4cdae4`, `sbb`).
        3 => after(a.deadline == b.deadline, a.deadline >= b.deadline),
        // The buyout, the highest first, unsigned, so no buyout sinks (`0x4cdaf2`-`0x4cdb01`).
        4 => after(
            a.buyout_price == b.buyout_price,
            a.buyout_price < b.buyout_price,
        ),
        // The bidder's rank, the higher first (`0x4cdb6a`-`0x4cdb7c`, `setl`).
        5 => after(
            a.bidder_rank == b.bidder_rank,
            a.bidder_rank < b.bidder_rank,
        ),
        // The template's name without case (`0x4cd9c1`-`0x4cd9ce`, `SStrCmpI` on `+8`).
        6 if cached => crate::crt::str_cmp_i(
            a.base_name.as_deref().unwrap_or(""),
            b.base_name.as_deref().unwrap_or(""),
        ),
        // The sellers' cached names without case; one the cache lacks after every named one
        // (`0x4cdc23` for a lone `b`, `0x4cdc31`-`0x4cdc37` for `a`).
        7 => match (&a.owner, &b.owner) {
            (Some(x), Some(y)) => crate::crt::str_cmp_i(x, y),
            (Some(_), None) => -1,
            (None, Some(_)) => 1,
            (None, None) => 0,
        },
        _ => 0,
    }
}

/// The level the level sort and `GetAuctionItemInfo` read: the requirement as a signed dword, at
/// least 1 (`0x4cda98`-`0x4cdab4`, `0x4cf091`-`0x4cf0a3`, `jl`), so 2^31 or more floors to 1.
pub(crate) fn floor_level(row: &AuctionItemRow) -> i32 {
    (row.level as i32).max(1)
}

/// The bid sort's key: the current bid, else the opening price (`0x4cdb81`-`0x4cdb9a`), as the
/// comparator's signed dword.
fn shown_bid(row: &AuctionItemRow) -> i32 {
    let bid = if row.bid_amount != 0 {
        row.bid_amount
    } else {
        row.min_bid
    };
    bid as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(stack: &AuctionSortStack) -> Vec<&'static str> {
        stack
            .slots()
            .iter()
            .map(|&(c, _)| SORT_KEYS[usize::from(c)])
            .collect()
    }

    /// The world entry's stacks (`0x4cc238`-`0x4cc274`).
    #[test]
    fn the_stacks_start_with_every_column_unreversed() {
        assert_eq!(
            columns(&AuctionSortStack::seeded(LIST)),
            ["level", "quality", "bid", "duration", "buyout", "status", "name", "seller"]
        );
        for list in [BIDDER, OWNER] {
            assert_eq!(
                columns(&AuctionSortStack::seeded(list)),
                ["status", "duration", "bid", "level", "name", "quality", "buyout", "seller"]
            );
        }
        assert!(AuctionSortStack::seeded(LIST)
            .slots()
            .iter()
            .all(|&(_, r)| !r));
    }

    /// The primary flips; another column comes to the front with its flag, the rest keep their
    /// order; the stack never grows or loses a column.
    #[test]
    fn a_click_flips_the_primary_and_promotes_any_other_with_its_flag() {
        let mut s = AuctionSortStack::seeded(LIST);
        s.promote(0);
        assert_eq!(s.reversed(0), Some(true), "level was the primary: it flips");
        s.promote(4);
        assert_eq!(
            columns(&s),
            ["buyout", "level", "quality", "bid", "duration", "status", "name", "seller"]
        );
        assert_eq!(s.reversed(4), Some(false));
        s.promote(0);
        assert_eq!(s.slots()[0], (0, true), "promoted with the flag it had");
        s.promote(9);
        assert_eq!(s.slots()[0], (0, true), "an unknown column changes nothing");
    }

    fn row(name: &str) -> AuctionItemRow {
        AuctionItemRow {
            name: Some(name.into()),
            base_name: Some(name.into()),
            quality: Some(1),
            ..Default::default()
        }
    }

    /// A row whose item record is not cached ties on level, quality and name, and is ordered by
    /// the other five like any row.
    #[test]
    fn an_uncached_row_ties_on_the_item_columns_only() {
        let cached = AuctionItemRow {
            level: 40,
            deadline: 10,
            ..row("Zed")
        };
        let uncached = AuctionItemRow {
            quality: None,
            base_name: None,
            deadline: 20,
            ..Default::default()
        };
        for column in [0, 1, 6] {
            assert_eq!(compare(column, &cached, &uncached), 0, "column {column}");
        }
        assert_eq!(
            compare(3, &cached, &uncached),
            -1,
            "the sooner deadline first"
        );
    }

    /// Each column's order at reversed 0, per its arm of `0x4cd940`.
    #[test]
    fn each_column_orders_as_its_arm_does() {
        let lo = AuctionItemRow {
            level: 0,
            quality: Some(1),
            min_bid: 100,
            deadline: 5,
            buyout_price: 0,
            bidder_rank: 0,
            ..row("apple")
        };
        let hi = AuctionItemRow {
            level: 2,
            quality: Some(4),
            min_bid: 50,
            bid_amount: 300,
            deadline: u32::MAX,
            buyout_price: 900,
            bidder_rank: 2,
            ..row("Banana")
        };
        assert_eq!(compare(0, &lo, &hi), -1, "level: the lowest first");
        assert_eq!(compare(1, &lo, &hi), 1, "quality: the highest first");
        assert_eq!(
            compare(2, &lo, &hi),
            -1,
            "bid: 100 against a current bid of 300"
        );
        assert_eq!(
            compare(3, &lo, &hi),
            -1,
            "duration: unsigned, the soonest first"
        );
        assert_eq!(
            compare(4, &lo, &hi),
            1,
            "buyout: the highest first, none last"
        );
        assert_eq!(compare(5, &lo, &hi), 1, "status: the higher rank first");
        assert_eq!(
            compare(6, &lo, &hi),
            -1,
            "name: apple before Banana without case"
        );
        let level_one = AuctionItemRow {
            level: 1,
            ..lo.clone()
        };
        assert_eq!(compare(0, &lo, &level_one), 0, "0 floors to 1");
    }

    /// The level is read as a signed dword and floored at 1 (`0x4cda98`-`0x4cdab4`, `jl`;
    /// `0x4cf091`-`0x4cf0a3`), so a level of 2^31 or more is negative and floors to 1.
    #[test]
    fn the_level_reads_signed_and_floors_at_one() {
        let at = |level: u32| AuctionItemRow { level, ..row("a") };
        for level in [0, 1, 0x8000_0000, 0xffff_ffff] {
            assert_eq!(floor_level(&at(level)), 1, "{level:#x}");
        }
        assert_eq!(floor_level(&at(0x7fff_ffff)), i32::MAX);
        assert_eq!(compare(0, &at(0x8000_0000), &at(1)), 0, "ties level 1");
        assert_eq!(
            compare(0, &at(0xffff_ffff), &at(2)),
            -1,
            "sorts below level 2"
        );
    }

    /// The bid is a signed dword in the compare (`setge`), so a bid past `i32::MAX` sorts low.
    #[test]
    fn the_bid_compares_signed() {
        let huge = AuctionItemRow {
            min_bid: 0x8000_0000,
            ..row("a")
        };
        let small = AuctionItemRow {
            min_bid: 1,
            ..row("b")
        };
        assert_eq!(compare(2, &huge, &small), -1);
    }

    /// A reversed slot negates its key, and a tie on one key falls to the next.
    #[test]
    fn the_stack_walks_its_keys_and_negates_a_reversed_one() {
        let mut stack = AuctionSortStack::seeded(LIST);
        let mut rows = vec![
            AuctionItemRow {
                level: 10,
                min_bid: 500,
                ..row("b")
            },
            AuctionItemRow {
                level: 10,
                min_bid: 100,
                ..row("a")
            },
            AuctionItemRow {
                level: 5,
                ..row("c")
            },
        ];
        stack.sort(&mut rows);
        let names = |rows: &[AuctionItemRow]| -> Vec<String> {
            rows.iter().map(|r| r.name.clone().unwrap()).collect()
        };
        assert_eq!(names(&rows), ["c", "a", "b"], "level, then bid");
        stack.promote(0);
        stack.sort(&mut rows);
        assert_eq!(
            names(&rows),
            ["a", "b", "c"],
            "level reversed, the bid still ascending"
        );
    }
}
