//! The guild roster's **view** — sorted, never filtered (the comparator `0x4d0d50`).
//!
//! Two things about it are the opposite of the obvious design, and both are verified at the bytes:
//!
//! - **The sort is multi-level.** The reference keeps an eight-slot `{key, direction}` chain
//!   (`0xb72680`), most-recently-chosen first, and its comparator walks the whole chain — so every
//!   column ever clicked survives as a tie-break behind the current one ([`SortStack`]).
//! - **Show-offline is a sort input, not a filter.** When it is off, the comparator runs a pre-gate
//!   that sinks offline members below online ones (`0x4d0d60`–`0x4d0d8a`); the array still holds
//!   every member. That pre-gate is exactly what makes `GetNumGuildMembers`' smaller count address
//!   the online prefix (see [`super::GuildState::num_members`]).

use std::cmp::Ordering;

use benilla_ui::script::GuildMemberInfo;

/// A column the guild roster can be ordered by — the eight `SortGuildRoster` accepts, in the
/// reference's own key numbering (`0x4d1cb0`'s string chain: `rank` 0, `level` 1, `name` 2,
/// `zone` 3, `class` 4, `group` 5, `online` 6, `note` 7). The numbering matters because the chain
/// is initialized to exactly `key[i] = i` (`0x4d0a50`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SortField {
    Rank,
    Level,
    Name,
    Zone,
    Class,
    /// **A dead key.** Its jump-table entry is the comparator loop's own continue label
    /// (`0x4d0f44`'s slot 5 → `0x4d0efd`): it reads no field and orders nothing. Kept rather than
    /// dropped because dropping it would silently re-map an addon's `"group"` onto `name`.
    Group,
    Online,
    Note,
}

/// The eight fields in key order — [`SortStack`]'s initial state, and the reference's own
/// `for (i = 0; i < 8; i++) { key[i] = i; dir[i] = 0; }` (`0x4d0a50`–`0x4d0a62`).
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
    /// The field a `SortGuildRoster(field)` string names. **Case-insensitive** (the reference's
    /// own collation `0x64a4c0` → `0x414310` folds `A-Z` with `+0x20` before comparing), and
    /// **anything unrecognised sorts by `name`** — the string chain leaves `edi` at its preloaded
    /// `mov edi,2` and calls with it anyway (`0x4d1cdb`, `0x4d1df4`). It neither raises nor no-ops.
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

    /// Order two rows by this column alone. `Equal` means "this column does not decide it", which
    /// is what the reference's arms mean when they jump to the loop tail.
    fn compare(self, a: &RosterRow, b: &RosterRow) -> Ordering {
        match self {
            // Rank is the one column ordered by the OPPOSITE of its number: `0x4d0e54 cmp ecx,eax`
            // puts b against a and the `sbb/and/inc` idiom answers −1 when `b.rank < a.rank`, so
            // the HIGHER rank id — the lowest authority — comes first and the guild master sorts
            // last. Every other numeric column is the plain a-vs-b order (`0x4d0dd3`'s `setge`).
            SortField::Rank => b.info.rank_index.cmp(&a.info.rank_index),
            SortField::Level => a.info.level.cmp(&b.info.level),
            SortField::Name => icmp(&a.info.name, &b.info.name),
            // Zone and class order by the resolved DBC *name*, not by the id (`0x4d0e67` and
            // `0x4d0ded` walk the id to a row and compare the localized string at
            // `+0x2c + locale*4` / `+0x14 + locale*4`) — and a row the DBC does not resolve makes
            // the column abstain rather than sort first (`0x4d0ea1`/`0x4d0e27 je` → the loop tail).
            SortField::Zone => abstaining(&a.info.zone, &b.info.zone),
            SortField::Class => abstaining(&a.info.class, &b.info.class),
            SortField::Group => Ordering::Equal,
            SortField::Online => match (a.info.online, b.info.online) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                // Both offline: the more recently seen first — the reference's `fld`/`fcomp` on
                // the raw days-since-logout float (`0x4d0f19`). It answers ±1 rather than 0 for an
                // exact tie, which we deliberately do not copy: Rust's sort demands a total order,
                // and a non-antisymmetric comparator is a panic here and a coin flip there.
                (false, false) => a
                    .last_online_days
                    .partial_cmp(&b.last_online_days)
                    .unwrap_or(Ordering::Equal),
            },
            SortField::Note => icmp(&a.info.note, &b.info.note),
        }
    }
}

/// Case-insensitive ASCII order, allocation-free — the reference's `0x414310` (fold `A-Z` by
/// `+0x20`, then compare bytes).
fn icmp(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// [`icmp`], except that an **empty** value on either side abstains instead of sorting first — the
/// DBC-miss leg of the zone and class arms.
fn abstaining(a: &str, b: &str) -> Ordering {
    if a.is_empty() || b.is_empty() {
        return Ordering::Equal;
    }
    icmp(a, b)
}

/// The eight-slot `{key, direction}` chain the reference keeps (`0x4d0fb0` maintains it,
/// `0x4d0d50` walks it), most-recently-chosen first.
///
/// `SortGuildRoster(field)` does one of two things:
/// - the field is already at slot 0 → **flip its direction** (`0x4d0fcf`'s `sete`);
/// - otherwise → **move it to slot 0**, shifting the rest down, keeping the direction it was last
///   left at (`0x4d0fdc`–`0x4d0ff3`).
///
/// The chain starts as the eight columns in key order, all ascending, so a column's *first* click
/// is ascending and its second reverses — but a column returned to after being reversed comes back
/// **reversed**, and the column clicked before it is still the tie-break. A one-column-plus-
/// direction model gets both of those wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SortStack([(SortField, bool); 8]);

impl Default for SortStack {
    fn default() -> Self {
        SortStack(SORT_FIELDS.map(|f| (f, false)))
    }
}

impl SortStack {
    /// Apply a `SortGuildRoster(field)` — see the type doc for the two cases.
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

    /// The current primary column and its direction — what the display order is *by*.
    #[cfg(test)]
    fn primary(&self) -> (SortField, bool) {
        self.0[0]
    }

    /// Order two rows. `show_offline` is a real input, not a filter: while it is **off**, an
    /// offline member sinks below every online one before any key is consulted (the pre-gate at
    /// `0x4d0d60`–`0x4d0d8a`, skipped entirely when it is on). Otherwise the first key on the
    /// chain that decides wins, with only *that* key's direction applied (`0x4d0f36`'s `neg eax`
    /// reads the deciding slot's flag, not slot 0's).
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

    /// Put `rows` in display order.
    ///
    /// `sort_unstable_by`, deliberately: the reference is the MSVC CRT `qsort` (`0x73f727`,
    /// median-of-3 with an insertion-sort cutoff), which is **not** stable. Adding a stability we
    /// do not have would diverge on ties rather than converge.
    pub(super) fn order(&self, rows: &mut [RosterRow], show_offline: bool) {
        rows.sort_unstable_by(|a, b| self.compare(a, b, show_offline));
    }
}

/// One roster row on its way to the VM: the display-resolved [`GuildMemberInfo`] the snapshot
/// carries, plus the two things the *app* still needs — the member's guid (the selection is a
/// guid, see [`super`]) and the raw days-since-logout the online column's tie-break compares.
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

    /// A repeat click reverses the column and a fresh column starts ascending — plus the half a
    /// `(field, direction)` pair cannot hold: a column returned to comes back **reversed**,
    /// because the reference remembers all eight directions.
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

    /// The chain is the tie-break: the column chosen *before* the current one still decides rows
    /// the current one ties on.
    #[test]
    fn the_previous_column_breaks_the_new_ones_ties() {
        let mut sort = SortStack::default();
        sort.select(SortField::Name); // then...
        sort.select(SortField::Level); // ...level on top, name behind it
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

    /// Every one of the reference's eight strings maps to its own column, matching is
    /// case-insensitive, and anything else sorts by name rather than raising or no-opping.
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

    /// `group` is accepted and orders nothing — the key under it decides.
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

    /// Rank sorts the guild master LAST at direction 0 — the one column whose byte order is the
    /// reverse of its number.
    #[test]
    fn rank_sorts_the_guild_master_last() {
        let mut sort = SortStack::default();
        sort.select(SortField::Rank);
        sort.select(SortField::Rank); // back to ascending (the default already had it at slot 0)
        let master = row("Tigole", true, 60, 0);
        let initiate = row("Kaplan", true, 60, 4);
        assert_eq!(sort.compare(&initiate, &master, true), Ordering::Less);
    }

    /// While show-offline is OFF the offline members sink to the bottom of the SAME array — which
    /// is what makes the smaller `GetNumGuildMembers` count address exactly the online prefix.
    /// While it is ON they interleave under the field keys.
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

    /// Between two offline members the online column breaks the tie by who was seen most
    /// recently, and a zone or class the DBC could not name abstains instead of sorting first.
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
