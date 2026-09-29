//! The `/who` sort (`SortWho` `0x5ad890`, comparator `0x5ada00`): a chain of seven `{key, dir}`
//! slots (`0xc2817c`/`0xc28180`), seeded zone, level, class, group, name, race, guild, all
//! ascending (`0x5add04`). A click promotes its key to the front and the comparator walks the chain
//! as tie-breakers, so earlier clicks still order ties. The direction flips only when the key was
//! already in front (`0x5ad99e`–`0x5ad9a7`); promoted from behind, it keeps its old direction.
//! Sorting is local, never a server request, and the chain lives for the process (`0x5adc50`, run
//! from `0x401666`), so it survives a logout.

use std::cmp::Ordering;

use super::social::WhoInfo;

/// The seven keys `SortWho` maps its argument to, declared in the reference's numbering
/// (`0x5ad8bb`–`0x5ad979`), which is also the chain's seeded order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhoSortKey {
    /// `0`, `"zone"`, the frame's resting column.
    Zone,
    /// `1`, `"level"`, the one numeric arm: ascending puts the lower level first.
    Level,
    /// `2`, `"class"`.
    Class,
    /// `3`, `"group"`: its arm (`0x5adbb2`) orders nothing, but promoting it still displaces the
    /// front key. The stock header for it is commented out (`FriendsFrame.xml:1413-1429`).
    Group,
    /// `4`, `"name"`, and the answer for any other string (preloaded at `0x5ad8bb`).
    #[default]
    Name,
    /// `5`, `"race"`.
    Race,
    /// `6`, `"guild"`.
    Guild,
}

impl WhoSortKey {
    /// Every key, in the chain's seeded order.
    const ALL: [Self; 7] = [
        Self::Zone,
        Self::Level,
        Self::Class,
        Self::Group,
        Self::Name,
        Self::Race,
        Self::Guild,
    ];

    /// `SortWho`'s argument to its key: seven ASCII case-insensitive compares (`0x64a4c0`,
    /// `0x5ad8c0`–`0x5ad979`), [`Self::Name`] when none match.
    pub fn from_sort_type(sort_type: &str) -> Self {
        match () {
            _ if sort_type.eq_ignore_ascii_case("name") => Self::Name,
            _ if sort_type.eq_ignore_ascii_case("level") => Self::Level,
            _ if sort_type.eq_ignore_ascii_case("class") => Self::Class,
            _ if sort_type.eq_ignore_ascii_case("group") => Self::Group,
            _ if sort_type.eq_ignore_ascii_case("race") => Self::Race,
            _ if sort_type.eq_ignore_ascii_case("zone") => Self::Zone,
            _ if sort_type.eq_ignore_ascii_case("guild") => Self::Guild,
            _ => Self::Name,
        }
    }

    /// This key's ascending order of two rows: the comparator's jump-table arms (`0x5adbd8`).
    fn order(self, a: &WhoInfo, b: &WhoInfo) -> Ordering {
        match self {
            // `a+0`/`a+0x30`, wire strings; an empty guild is a real value and sorts first.
            Self::Name => ascii_ci_cmp(&a.name, &b.name),
            Self::Guild => ascii_ci_cmp(&a.guild, &b.guild),
            // `a+0x90`, an i32, lower level first.
            Self::Level => a.level.cmp(&b.level),
            // The localized `ChrClasses`/`ChrRaces`/`AreaTable` name the app resolved into the row.
            Self::Class => dbc_name_cmp(&a.class, &b.class),
            Self::Race => dbc_name_cmp(&a.race, &b.race),
            Self::Zone => dbc_name_cmp(&a.zone, &b.zone),
            // `0x5adbb2` is the chain loop's continue label.
            Self::Group => Ordering::Equal,
        }
    }
}

/// The seven-slot chain: keys most recently used first, each with the direction it was left in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhoSortChain {
    /// `(key, reversed)`, slot 0 first; always a permutation of all seven keys.
    slots: [(WhoSortKey, bool); 7],
}

impl Default for WhoSortChain {
    /// The initialiser `0x5add04`–`0x5add16`: `key[i] = i`, `dir[i] = 0`.
    fn default() -> Self {
        Self {
            slots: WhoSortKey::ALL.map(|key| (key, false)),
        }
    }
}

impl WhoSortChain {
    /// `SortWho`'s promote (`0x5ad980`–`0x5ad9c9`): move the key to the front, flipping its
    /// direction only if it was already there; the keys it passes shift down one slot.
    pub fn promote(&mut self, sort_type: &str) {
        let key = WhoSortKey::from_sort_type(sort_type);
        let Some(at) = self.slots.iter().position(|(k, _)| *k == key) else {
            // Unreachable; the reference's not-found edge skips the promote and sorts (`0x5ad991`).
            return;
        };
        let dir = if at == 0 {
            !self.slots[0].1
        } else {
            self.slots[at].1
        };
        self.slots[..=at].rotate_right(1);
        self.slots[0] = (key, dir);
    }

    /// The comparator (`0x5ada00`): the first key that does not tie, negated when its slot is
    /// reversed, else `Equal`.
    pub fn compare(&self, a: &WhoInfo, b: &WhoInfo) -> Ordering {
        for (key, reversed) in self.slots {
            let order = key.order(a, b);
            if order != Ordering::Equal {
                return if reversed { order.reverse() } else { order };
            }
        }
        Ordering::Equal
    }

    /// Order `rows` by the chain, as the reference's `qsort` does on a click (`0x5ad9cf`) and on
    /// each `SMSG_WHO` answer (`0x5ae0e2`). This sort is stable where `qsort` is not, which no two
    /// rows can show: names are unique and the name key never leaves the chain.
    pub fn sort(&self, rows: &mut [WhoInfo]) {
        rows.sort_by(|a, b| self.compare(a, b));
    }

    #[cfg(test)]
    fn keys(&self) -> [(WhoSortKey, bool); 7] {
        self.slots
    }
}

/// The reference's string compare (`0x64a4c0`, `0x414310`): it folds `A`–`Z` only
/// (`0x41434a`–`0x41435c`), so accented names order as they do there, not by a Unicode fold.
fn ascii_ci_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    for (x, y) in a.iter().zip(b) {
        match x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    // The reference compares NUL-terminated strings, so the shorter one wins its own prefix.
    a.len().cmp(&b.len())
}

/// The class, race and zone arms (`0x5ada7b`, `0x5adadf`, `0x5adb40`): an id with no DBC row, an
/// empty name here, sorts after every resolved name, not first as `""`. Two misses tie and pass
/// to the next key (`0x5adbb2`), as there. The empty name is the miss marker: the reference's
/// `GetWhoInfo` (`0x5ad6e0`) shows `UNKNOWN` for it, benilla's an empty cell, and a row that
/// carried `UNKNOWN` would sort on that word.
fn dbc_name_cmp(a: &str, b: &str) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        // Deviation: the reference ties a miss against a hit (`0x5adab6`); no total order can, and
        // `sort_by` may panic on one that is not, so the miss goes last.
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => ascii_ci_cmp(a, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, level: u32, class: &str, zone: &str) -> WhoInfo {
        WhoInfo {
            name: name.to_string(),
            guild: String::new(),
            level,
            race: "Human".to_string(),
            class: class.to_string(),
            zone: zone.to_string(),
        }
    }

    fn names(rows: &[WhoInfo]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn the_chain_is_seeded_in_key_order() {
        assert_eq!(
            WhoSortChain::default().keys(),
            [
                (WhoSortKey::Zone, false),
                (WhoSortKey::Level, false),
                (WhoSortKey::Class, false),
                (WhoSortKey::Group, false),
                (WhoSortKey::Name, false),
                (WhoSortKey::Race, false),
                (WhoSortKey::Guild, false),
            ]
        );
    }

    #[test]
    fn the_sort_type_maps_case_insensitively_and_falls_back_to_name() {
        for (arg, key) in [
            ("zone", WhoSortKey::Zone),
            ("level", WhoSortKey::Level),
            ("class", WhoSortKey::Class),
            ("group", WhoSortKey::Group),
            ("name", WhoSortKey::Name),
            ("race", WhoSortKey::Race),
            ("guild", WhoSortKey::Guild),
            ("LEVEL", WhoSortKey::Level),
            ("Guild", WhoSortKey::Guild),
        ] {
            assert_eq!(WhoSortKey::from_sort_type(arg), key, "{arg}");
        }
        for arg in ["", "sortme", "5", "levels"] {
            assert_eq!(
                WhoSortKey::from_sort_type(arg),
                WhoSortKey::Name,
                "{arg} must fall back to name"
            );
        }
    }

    #[test]
    fn a_repeated_click_reverses() {
        let mut rows = vec![
            row("Galas", 60, "Warrior", "Elwynn Forest"),
            row("Erdrin", 60, "Warrior", "Elwynn Forest"),
        ];
        let mut chain = WhoSortChain::default();

        chain.promote("name");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Erdrin", "Galas"], "first click ascends");

        chain.promote("name");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Galas", "Erdrin"], "the repeat reverses");

        chain.promote("name");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Erdrin", "Galas"], "and back again");
    }

    #[test]
    fn promoting_from_behind_carries_the_direction_it_left() {
        let mut chain = WhoSortChain::default();
        chain.promote("name"); // ascending
        chain.promote("name"); // descending
        chain.promote("level"); // name drops to slot 1, still descending
        assert_eq!(chain.keys()[1], (WhoSortKey::Name, true));

        chain.promote("name"); // back to the front, not a flip
        assert_eq!(chain.keys()[0], (WhoSortKey::Name, true));

        let mut rows = vec![
            row("Aaa", 1, "Mage", "Durotar"),
            row("Bbb", 1, "Mage", "Durotar"),
        ];
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Bbb", "Aaa"], "still descending");
    }

    #[test]
    fn earlier_clicks_survive_as_tie_breakers() {
        let mut chain = WhoSortChain::default();
        chain.promote("level");
        chain.promote("class");
        assert_eq!(
            chain.keys()[..2],
            [(WhoSortKey::Class, false), (WhoSortKey::Level, false)]
        );

        let mut rows = vec![
            row("Ca", 40, "Rogue", "Westfall"),
            row("Aa", 60, "Mage", "Westfall"),
            row("Ba", 20, "Rogue", "Westfall"),
        ];
        chain.sort(&mut rows);
        assert_eq!(
            names(&rows),
            ["Aa", "Ba", "Ca"],
            "Mage before Rogue; the two Rogues in the level order the earlier click gave them"
        );
    }

    #[test]
    fn level_ascends_before_it_reverses() {
        let mut rows = vec![row("High", 60, "Mage", "Z"), row("Low", 12, "Mage", "Z")];
        let mut chain = WhoSortChain::default();
        chain.promote("level");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Low", "High"]);
        chain.promote("level");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["High", "Low"]);
    }

    #[test]
    fn the_string_arms_fold_ascii_case() {
        assert_eq!(ascii_ci_cmp("Apple", "apple"), Ordering::Equal);
        assert_eq!(ascii_ci_cmp("apple", "Zed"), Ordering::Less);
        assert_eq!(ascii_ci_cmp("Zed", "apple"), Ordering::Greater);
        assert_eq!(ascii_ci_cmp("Ann", "Anne"), Ordering::Less, "prefix first");
    }

    #[test]
    fn an_unresolved_dbc_name_sorts_after_the_resolved_ones() {
        // Zone is the seeded front key, so the promote flips it: the default chain already ascends.
        let chain = WhoSortChain::default();
        let known = row("Bbb", 10, "Mage", "Westfall");
        let unknown = row("Aaa", 10, "Mage", "");
        assert_eq!(chain.compare(&known, &unknown), Ordering::Less);
        assert_eq!(chain.compare(&unknown, &known), Ordering::Greater);
        let other_unknown = row("Ccc", 10, "Mage", "");
        assert_eq!(
            chain.compare(&unknown, &other_unknown),
            Ordering::Less,
            "two misses tie on zone, so the name decides"
        );

        // A guild is not a DBC lookup: an empty guild is a real value and sorts first.
        assert_eq!(ascii_ci_cmp("", "Legacy"), Ordering::Less);
    }

    type Setter = fn(&mut WhoInfo, &str);

    /// Each DBC-backed key as a single-key comparator, with the setter for its field.
    fn dbc_keys() -> [(&'static str, Setter); 3] {
        [
            ("class", |r, v| r.class = v.to_string()),
            ("race", |r, v| r.race = v.to_string()),
            ("zone", |r, v| r.zone = v.to_string()),
        ]
    }

    fn mixed_rows(set: Setter, n: usize) -> Vec<WhoInfo> {
        let values = [
            "",
            "Mage",
            "Warrior",
            "",
            "Elwynn Forest",
            "Durotar",
            "Mage",
        ];
        (0..n)
            .map(|i| {
                let mut r = row(&format!("N{i:03}"), 1 + (i % 7) as u32, "Mage", "Z");
                set(&mut r, values[(i * 5 + i / 7) % values.len()]);
                r
            })
            .collect()
    }

    #[test]
    fn the_dbc_arms_are_a_total_order() {
        for (key, set) in dbc_keys() {
            let key = WhoSortKey::from_sort_type(key);
            let rows = mixed_rows(set, 21);
            let le = |a: &WhoInfo, b: &WhoInfo| key.order(a, b) != Ordering::Greater;
            for a in &rows {
                for b in &rows {
                    assert_eq!(key.order(a, b), key.order(b, a).reverse(), "{key:?}");
                    for c in &rows {
                        if le(a, b) && le(b, c) {
                            assert!(le(a, c), "{key:?} not transitive");
                        }
                        if key.order(a, b) == Ordering::Equal && key.order(b, c) == Ordering::Equal
                        {
                            assert_eq!(key.order(a, c), Ordering::Equal, "{key:?} tie");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sorting_a_mixed_list_puts_misses_last_without_panicking() {
        for (key, set) in dbc_keys() {
            let mut chain = WhoSortChain::default();
            chain.promote("level"); // so the key below is promoted from behind, ascending
            chain.promote(key);
            let mut rows = mixed_rows(set, 64);
            chain.sort(&mut rows);
            let field = |r: &WhoInfo| match key {
                "class" => r.class.clone(),
                "race" => r.race.clone(),
                _ => r.zone.clone(),
            };
            let first_miss = rows.iter().position(|r| field(r).is_empty()).unwrap();
            assert!(
                rows[first_miss..].iter().all(|r| field(r).is_empty()),
                "{key}"
            );
            assert!(rows[..first_miss]
                .windows(2)
                .all(|w| { ascii_ci_cmp(&field(&w[0]), &field(&w[1])) != Ordering::Greater }));
        }
    }

    #[test]
    fn the_group_key_displaces_but_orders_nothing() {
        let mut chain = WhoSortChain::default();
        chain.promote("level");
        chain.promote("group");
        assert_eq!(
            chain.keys()[..2],
            [(WhoSortKey::Group, false), (WhoSortKey::Level, false)]
        );
        let mut rows = vec![row("A", 60, "Mage", "Z"), row("B", 12, "Mage", "Z")];
        chain.sort(&mut rows);
        assert_eq!(
            names(&rows),
            ["B", "A"],
            "ordered by level, group having tied"
        );
    }

    #[test]
    fn identical_rows_tie_on_every_key() {
        let chain = WhoSortChain::default();
        let a = row("Same", 30, "Priest", "Ironforge");
        assert_eq!(chain.compare(&a, &a.clone()), Ordering::Equal);
    }
}
