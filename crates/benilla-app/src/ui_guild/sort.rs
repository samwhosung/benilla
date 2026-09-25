//! The guild roster's display order: sorted, never filtered (the reference comparator
//! `0x4d0d50`). With show-offline off, offline members sink below online ones
//! (`0x4d0d60`-`0x4d0d8a`), so `GetNumGuildMembers`' online count addresses the leading rows.

use std::cmp::Ordering;

use benilla_ui::script::GuildMemberInfo;

/// A `SortGuildRoster` column, in the reference's key order (`0x4d1cb0`): `rank` 0, `level` 1,
/// `name` 2, `zone` 3, `class` 4, `group` 5, `online` 6, `note` 7. The chain starts as
/// `key[i] = i` (`0x4d0a50`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SortField {
    Rank,
    Level,
    Name,
    Zone,
    Class,
    /// A dead key: its jump-table slot (`0x4d0f44`, slot 5) is the loop's continue label
    /// `0x4d0efd`, so it orders nothing. It stays so `"group"` does not parse as `name`.
    Group,
    Online,
    Note,
}

/// [`SortStack`]'s initial chain: the eight keys in order, all ascending (`0x4d0a50`-`0x4d0a62`).
const SORT_FIELDS: [SortField; 8] = [
    SortField::Rank,
    SortField::Level,
    SortField::Name,
    SortField::Zone,
    SortField::Class,
    SortField::Group,
    SortField::Online,
    SortField::Note,
];

impl SortField {
    /// The column a `SortGuildRoster` string names, case-insensitively (`0x64a4c0`, `0x414310`);
    /// anything else sorts by `name`, the preloaded key (`0x4d1cdb`, `0x4d1df4`).
    pub(super) fn parse(field: &str) -> SortField {
        match field {
            f if f.eq_ignore_ascii_case("rank") => SortField::Rank,
            f if f.eq_ignore_ascii_case("level") => SortField::Level,
            f if f.eq_ignore_ascii_case("zone") => SortField::Zone,
            f if f.eq_ignore_ascii_case("class") => SortField::Class,
            f if f.eq_ignore_ascii_case("group") => SortField::Group,
            f if f.eq_ignore_ascii_case("online") => SortField::Online,
            f if f.eq_ignore_ascii_case("note") => SortField::Note,
            _ => SortField::Name,
        }
    }

    /// Order two rows by this column alone; `Equal` is the reference arm's jump to the loop tail.
    fn compare(self, a: &RosterRow, b: &RosterRow) -> Ordering {
        match self {
            // Reversed: the higher rank id sorts first, so the guild master sorts last
            // (`0x4d0e54`); every other numeric column is plain a-vs-b (`0x4d0dd3`).
            SortField::Rank => b.info.rank_index.cmp(&a.info.rank_index),
            SortField::Level => a.info.level.cmp(&b.info.level),
            SortField::Name => icmp(&a.info.name, &b.info.name),
            // By the localized DBC name, not the id (`0x4d0e67`, `0x4d0ded`); an id the DBC does
            // not resolve abstains rather than sorting first (`0x4d0ea1`, `0x4d0e27`).
            SortField::Zone => abstaining(&a.info.zone, &b.info.zone),
            SortField::Class => abstaining(&a.info.class, &b.info.class),
            SortField::Group => Ordering::Equal,
            SortField::Online => match (a.info.online, b.info.online) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                // Both offline: the more recently seen first, on the raw days float (`0x4d0f19`).
                // Deviation: an exact tie is `Equal` where the reference answers ±1, because
                // Rust's sort requires a total order.
                (false, false) => a
                    .last_online_days
                    .partial_cmp(&b.last_online_days)
                    .unwrap_or(Ordering::Equal),
            },
            SortField::Note => icmp(&a.info.note, &b.info.note),
        }
    }
}

/// Case-insensitive ASCII order, as the reference's `0x414310` folds `A-Z` and compares bytes.
fn icmp(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// [`icmp`], but an empty value on either side abstains: the zone and class arms' DBC miss.
fn abstaining(a: &str, b: &str) -> Ordering {
    if a.is_empty() || b.is_empty() {
        return Ordering::Equal;
    }
    icmp(a, b)
}

/// The reference's eight-slot `{key, direction}` chain (`0xb72680`), most recent first: `0x4d0fb0`
/// maintains it and `0x4d0d50` walks it, so every column clicked before stays a tie-break, and one
/// returned to keeps the direction it was left at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SortStack([(SortField, bool); 8]);

impl Default for SortStack {
    fn default() -> Self {
        SortStack(SORT_FIELDS.map(|f| (f, false)))
    }
}

impl SortStack {
    /// `SortGuildRoster(field)`: a field already first flips its direction (`0x4d0fcf`); any other
    /// moves to the front with the direction it was left at (`0x4d0fdc`-`0x4d0ff3`).
    pub(super) fn select(&mut self, field: SortField) {
        let Some(at) = self.0.iter().position(|(f, _)| *f == field) else {
            return; // unreachable: the chain holds all eight, always
        };
        if at == 0 {
            self.0[0].1 = !self.0[0].1;
            return;
        }
        let moved = self.0[at];
        self.0.copy_within(0..at, 1);
        self.0[0] = moved;
    }

    /// The first column on the chain and whether it is descending.
    #[cfg(test)]
    fn primary(&self) -> (SortField, bool) {
        self.0[0]
    }

    /// Order two rows. With `show_offline` off, offline sinks below online before any key
    /// (`0x4d0d60`-`0x4d0d8a`); then the first deciding key wins, in its own direction, not slot
    /// 0's (`0x4d0f36`).
    fn compare(&self, a: &RosterRow, b: &RosterRow, show_offline: bool) -> Ordering {
        if !show_offline {
            match (a.info.online, b.info.online) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }
        for (field, descending) in self.0 {
            let ord = field.compare(a, b);
            if ord != Ordering::Equal {
                return if descending { ord.reverse() } else { ord };
            }
        }
        Ordering::Equal
    }

    /// Put `rows` in display order; unstable, like the reference's MSVC CRT `qsort` (`0x73f727`).
    pub(super) fn order(&self, rows: &mut [RosterRow], show_offline: bool) {
        rows.sort_unstable_by(|a, b| self.compare(a, b, show_offline));
    }
}

/// One roster row: the resolved [`GuildMemberInfo`] the VM gets, plus the guid the selection keys
/// on and the raw days-since-logout the online column's tie-break compares.
#[derive(Clone, Debug, Default)]
pub(crate) struct RosterRow {
    pub(crate) guid: u64,
    pub(crate) last_online_days: f32,
    pub(crate) info: GuildMemberInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, online: bool, level: u32, rank_index: u32) -> RosterRow {
        RosterRow {
            guid: 0,
            last_online_days: 0.0,
            info: GuildMemberInfo {
                name: name.to_string(),
                rank_index,
                level,
                online,
                ..Default::default()
            },
        }
    }

    #[test]
    fn a_repeat_column_reverses_and_a_returned_one_keeps_its_direction() {
        let mut sort = SortStack::default();
        sort.select(SortField::Name);
        assert_eq!(sort.primary(), (SortField::Name, false), "first click");
        sort.select(SortField::Name);
        assert_eq!(sort.primary(), (SortField::Name, true), "reversed");

        sort.select(SortField::Level);
        assert_eq!(
            sort.primary(),
            (SortField::Level, false),
            "a column not clicked before starts ascending"
        );

        sort.select(SortField::Name);
        assert_eq!(
            sort.primary(),
            (SortField::Name, true),
            "and one returned to keeps the direction it was left at — NOT a reset to ascending"
        );
    }

    #[test]
    fn the_previous_column_breaks_the_new_ones_ties() {
        let mut sort = SortStack::default();
        sort.select(SortField::Name);
        sort.select(SortField::Level); // level first, name behind it
        let a = row("Alice", true, 10, 0);
        let b = row("Bob", true, 10, 0);
        assert_eq!(
            sort.compare(&a, &b, true),
            Ordering::Less,
            "same level → by name"
        );
        assert_eq!(
            sort.compare(&row("Zed", true, 5, 0), &a, true),
            Ordering::Less,
            "different level → by level, and the name is never consulted"
        );
    }

    #[test]
    fn the_sort_field_names_are_the_references_eight() {
        for (text, field) in [
            ("rank", SortField::Rank),
            ("level", SortField::Level),
            ("name", SortField::Name),
            ("zone", SortField::Zone),
            ("class", SortField::Class),
            ("group", SortField::Group),
            ("online", SortField::Online),
            ("note", SortField::Note),
            ("ZONE", SortField::Zone),
            ("nonsense", SortField::Name),
            ("", SortField::Name),
        ] {
            assert_eq!(SortField::parse(text), field, "{text:?}");
        }
    }

    #[test]
    fn the_group_key_is_dead() {
        let mut sort = SortStack::default();
        sort.select(SortField::Level);
        sort.select(SortField::Group);
        assert_eq!(sort.primary(), (SortField::Group, false));
        assert_eq!(
            sort.compare(&row("A", true, 5, 0), &row("B", true, 9, 0), true),
            Ordering::Less,
            "level, the key it was promoted over, still decides"
        );
    }

    #[test]
    fn rank_sorts_the_guild_master_last() {
        let mut sort = SortStack::default();
        sort.select(SortField::Rank);
        sort.select(SortField::Rank); // back to ascending: rank starts at slot 0
        let master = row("Tigole", true, 60, 0);
        let initiate = row("Kaplan", true, 60, 4);
        assert_eq!(sort.compare(&initiate, &master, true), Ordering::Less);
    }

    #[test]
    fn show_offline_sinks_rather_than_filters() {
        let mut sort = SortStack::default();
        sort.select(SortField::Name);
        let mut rows = vec![
            row("Alice", false, 60, 0),
            row("Bob", true, 60, 0),
            row("Carol", false, 60, 0),
            row("Dave", true, 60, 0),
        ];

        sort.order(&mut rows, false);
        let names: Vec<&str> = rows.iter().map(|r| r.info.name.as_str()).collect();
        assert_eq!(names, ["Bob", "Dave", "Alice", "Carol"], "online prefix");
        assert_eq!(rows.len(), 4, "nothing is removed, ever");

        sort.order(&mut rows, true);
        let names: Vec<&str> = rows.iter().map(|r| r.info.name.as_str()).collect();
        assert_eq!(names, ["Alice", "Bob", "Carol", "Dave"], "interleaved");
    }

    #[test]
    fn the_online_tiebreak_and_the_dbc_miss_abstention() {
        let mut sort = SortStack::default();
        sort.select(SortField::Online);
        let recent = RosterRow {
            last_online_days: 0.5,
            ..row("Recent", false, 60, 0)
        };
        let ancient = RosterRow {
            last_online_days: 90.0,
            ..row("Ancient", false, 60, 0)
        };
        assert_eq!(sort.compare(&recent, &ancient, true), Ordering::Less);

        assert_eq!(abstaining("", "Ironforge"), Ordering::Equal);
        assert_eq!(abstaining("Ironforge", ""), Ordering::Equal);
        assert_eq!(abstaining("Elwynn", "Ironforge"), Ordering::Less);
        assert_eq!(icmp("alice", "Alice"), Ordering::Equal, "case-insensitive");
    }
}
