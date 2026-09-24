//! The `/who` list's sort — the reference's seven-slot key chain, its promote-and-flip rule, and
//! the comparator that walks it (`SortWho 0x5ad890`, comparator `0x5ada00`; benilla decision 2030).
//!
//! **A `/who` sort is not "order by the clicked column".** The reference keeps a *chain* of seven
//! `{key, dir}` slots at `0xc2817c`/`0xc28180`, seeded once per process to `key[i] = i`
//! (`0x5add04`), i.e. **zone → level → class → group → name → race → guild**, every direction
//! ascending. Each click *promotes* its key to the front of that chain; the keys it displaces
//! stay behind it, so the chain reads most-recently-used first and the comparator walks it as a
//! **tie-breaker list**, not as a single key. Two players on the same level keep whatever order
//! the previously clicked key gave them.
//!
//! Two consequences that are the whole of bug B365:
//!
//! - **A repeated click reverses.** The promote flips the direction **only when the key was
//!   already at the front** (`0x5ad99e`–`0x5ad9a7`, the `sete` on `i == 0`); promoting a key from
//!   anywhere else carries its *remembered* direction over unchanged. So Name, Name reverses; but
//!   Name, Level, Name does **not** — the second Name click restores the direction the first one
//!   left there.
//! - **Nothing here is a request to the server.** The chain and the comparator are engine state;
//!   `SortWho` sorts the array it already holds and fires `WHO_LIST_UPDATE` synchronously, inside
//!   the binding ([`super::social`]'s `SortWho`).
//!
//! The chain is **per-process** in the reference — its initialiser `0x5adc50` runs once from the
//! process-start run at `0x401666` — so it survives a logout and the next login inherits it.
//! `SocialState::clear_session` on the app side keeps it for that reason.

use std::cmp::Ordering;

use super::social::WhoInfo;

/// The seven keys `SortWho` maps its argument to. **The declaration order is the reference's key
/// numbering** (`edi` in `0x5ad8bb`–`0x5ad979`) *and* the chain's seeded order, because the
/// initialiser writes `key[i] = i`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhoSortKey {
    /// `0` — `"zone"`, the frame's own resting column.
    Zone,
    /// `1` — `"level"`, the one numeric arm: at `dir = 0` the **lower** level comes first.
    Level,
    /// `2` — `"class"`.
    Class,
    /// `3` — `"group"`. A real chain key whose comparator arm (`0x5adbb2`) *is* the continue
    /// label: it orders nothing, ever. Unreachable from the shipped UI as well — the header that
    /// carries `sortType = "group"` sits inside `FriendsFrame.xml`'s commented span 1413–1429 —
    /// but it is modelled because it occupies a slot, and promoting it still displaces whatever
    /// key was at the front.
    Group,
    /// `4` — `"name"`, and the answer for **any** string the seven compares miss: `edi` is
    /// preloaded with 4 at `0x5ad8bb` and never reassigned on the no-match path.
    #[default]
    Name,
    /// `5` — `"race"`.
    Race,
    /// `6` — `"guild"`.
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

    /// `SortWho`'s argument → key: seven `0x64a4c0` compares (`0x5ad8c0`–`0x5ad979`), which are
    /// **case-insensitive** (`0x414310` folds `A`–`Z` on both operands), with [`Self::Name`] as
    /// the no-match answer.
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

    /// This key's own ordering of two rows at `dir = 0` — the comparator's seven jump-table arms
    /// (`0x5adbd8`), each returning `0x64a4c0`'s `−1/0/+1` or the level arm's `setge` sign.
    fn order(self, a: &WhoInfo, b: &WhoInfo) -> Ordering {
        match self {
            // `a+0` / `a+0x30`: wire strings, compared case-insensitively. An empty guild is a
            // real value here (`""` sorts before any name), not a missing row.
            Self::Name => ascii_ci_cmp(&a.name, &b.name),
            Self::Guild => ascii_ci_cmp(&a.guild, &b.guild),
            // `a+0x90`, i32: `cmp; je tie; setge dl; lea edx,[edx*2-1]` — lower level first.
            Self::Level => a.level.cmp(&b.level),
            // The three DBC arms: the *localized name* out of `ChrClasses`/`ChrRaces`/`AreaTable`
            // is what gets compared, which is exactly what the app already resolved into the row.
            Self::Class => dbc_name_cmp(&a.class, &b.class),
            Self::Race => dbc_name_cmp(&a.race, &b.race),
            Self::Zone => dbc_name_cmp(&a.zone, &b.zone),
            // `0x5adbb2` is the continue label itself.
            Self::Group => Ordering::Equal,
        }
    }
}

/// The seven-slot `{key, dir}` chain at `0xc2817c`/`0xc28180`: the sort keys most-recently-used
/// first, each with the direction it was last left in. See the module doc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhoSortChain {
    /// `(key, reversed)`, slot 0 first. Every key appears exactly once — a promote permutes, it
    /// never inserts or drops.
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
    /// `SortWho`'s promote (`0x5ad980`–`0x5ad9c9`): move `sort_type`'s key to the front, flipping
    /// its direction **only if it was already there**, and shift the keys it passed down one slot
    /// each. Keys behind it keep their own remembered directions.
    pub fn promote(&mut self, sort_type: &str) {
        let key = WhoSortKey::from_sort_type(sort_type);
        let Some(at) = self.slots.iter().position(|(k, _)| *k == key) else {
            // Unreachable: the chain is a permutation of all seven keys. The reference's own
            // not-found edge (`0x5ad991 jmp 0x5ad9cf`) skips the promote and sorts anyway.
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

    /// The comparator `0x5ada00`: walk the chain, return the first key that does not tie —
    /// negated when that slot's direction is set — and `Equal` when all seven tie.
    pub fn compare(&self, a: &WhoInfo, b: &WhoInfo) -> Ordering {
        for (key, reversed) in self.slots {
            let order = key.order(a, b);
            if order != Ordering::Equal {
                return if reversed { order.reverse() } else { order };
            }
        }
        Ordering::Equal
    }

    /// Order `rows` by the chain — the `qsort` at `0x5ad9cf` (a click) and `0x5ae0e2` (a fresh
    /// `SMSG_WHO` answer, which is why results arrive already in the current order).
    ///
    /// The reference's `qsort` is unstable and this is not; the difference is unobservable,
    /// because the name key never leaves the chain and no two rows of one realm's `/who` answer
    /// share a name, so [`Self::compare`] is a total order over the rows that can actually occur.
    pub fn sort(&self, rows: &mut [WhoInfo]) {
        rows.sort_by(|a, b| self.compare(a, b));
    }

    /// The chain's keys, front (most recently clicked) first — for tests and diagnostics.
    #[cfg(test)]
    fn keys(&self) -> [(WhoSortKey, bool); 7] {
        self.slots
    }
}

/// `0x64a4c0` → `0x414310`: a **case-insensitive ASCII** compare, folding only `A`–`Z` by `+0x20`
/// on both operands (`0x41434a`–`0x41435c`) and answering `−1/0/+1`.
///
/// Deliberately not `to_lowercase()`: that is a Unicode fold, and it would order the accented
/// names the localized clients allow differently from the reference, which only ever touches the
/// 26 ASCII letters.
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

/// The three DBC-backed arms' compare. **A row the id could not resolve TIES** — verified on all
/// three arms (class `0x5ada7b`, race `0x5adadf`, zone `0x5adb40`), which are one idiom
/// byte-for-byte: three miss conditions (a negative id, an id past the *inclusive* bound, a NULL
/// row at an in-range id) zero the row register, and either side being zero jumps to `0x5adbb2`,
/// which **is** the chain loop's `inc esi`. So it is not a returned `0` compared against
/// something — the key contributes nothing and the next slot decides. `dir` cannot reach it
/// either (the `neg` is gated behind the compare the tie branch skips).
///
/// The tie matters more than it looks: an unresolved id would otherwise sort as `""`, parking
/// every unknown zone at the top of the list ahead of Ahn'Qiraj.
///
/// **The empty string is this side's marker for "no row", and the two sides of the reference
/// disagree on purpose.** `GetWhoInfo 0x5ad6e0` substitutes the localized `"UNKNOWN"` on the same
/// three legs, so a row can read UNKNOWN in the cell while sorting as though that column did not
/// exist. Anything that starts showing UNKNOWN here has to carry the miss to this comparator by
/// some route other than the string.
fn dbc_name_cmp(a: &str, b: &str) -> Ordering {
    if a.is_empty() || b.is_empty() {
        return Ordering::Equal;
    }
    ascii_ci_cmp(a, b)
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

    /// The seeded chain is the reference's `key[i] = i`, all ascending — so a list nobody has
    /// clicked is ordered zone, then level, then class, then name.
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

    /// The key mapping, including the case-insensitive compares and the name fallback that
    /// swallows every unrecognised string.
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

    /// **B365's headline.** A first click sorts ascending; the *same* key clicked again reverses.
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

    /// The flip is `i == 0` ONLY. Clicking a different key in between promotes name from slot 1
    /// with its remembered direction *carried*, so the third click below does not re-reverse —
    /// it restores what the second one left, which is the half a naive "toggle on every click"
    /// implementation gets wrong.
    #[test]
    fn promoting_from_behind_carries_the_direction_it_left() {
        let mut chain = WhoSortChain::default();
        chain.promote("name"); // ascending
        chain.promote("name"); // descending
        chain.promote("level"); // name drops to slot 1, still descending
        assert_eq!(chain.keys()[1], (WhoSortKey::Name, true));

        chain.promote("name"); // back to the front — NOT a flip
        assert_eq!(chain.keys()[0], (WhoSortKey::Name, true));

        let mut rows = vec![
            row("Aaa", 1, "Mage", "Durotar"),
            row("Bbb", 1, "Mage", "Durotar"),
        ];
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Bbb", "Aaa"], "still descending");
    }

    /// The displaced keys stay behind the clicked one, most-recently-used first — so a click is a
    /// tie-breaker chain, not a fresh single-key sort. Level, then class: rows equal on class
    /// keep the *level* order the earlier click established.
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

    /// The level arm is the numeric one, and ascending means the LOWER level first.
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

    /// Names fold case the way `0x414310` does — `apple` and `Apple` compare equal, and `Zed`
    /// sorts after `apple` rather than before it as a byte compare would have it.
    #[test]
    fn the_string_arms_fold_ascii_case() {
        assert_eq!(ascii_ci_cmp("Apple", "apple"), Ordering::Equal);
        assert_eq!(ascii_ci_cmp("apple", "Zed"), Ordering::Less);
        assert_eq!(ascii_ci_cmp("Zed", "apple"), Ordering::Greater);
        assert_eq!(ascii_ci_cmp("Ann", "Anne"), Ordering::Less, "prefix first");
    }

    /// An unresolved DBC name ties rather than sorting as `""` — otherwise every row whose zone
    /// id has no `AreaTable` row would pile up at the top of a zone sort.
    #[test]
    fn an_unresolved_dbc_name_ties() {
        let mut chain = WhoSortChain::default();
        chain.promote("zone");
        let known = row("Bbb", 10, "Mage", "Westfall");
        let unknown = row("Aaa", 10, "Mage", "");
        // Not `Less`: a `""` zone does not sort first, it does not sort at all — the zone key
        // ties and the chain falls through to the name, where Aaa beats Bbb.
        assert_eq!(
            chain.compare(&known, &unknown),
            Ordering::Greater,
            "the zone key ties, so the name decides"
        );
        assert_eq!(chain.compare(&unknown, &known), Ordering::Less);

        // A *guild* is not a DBC lookup: an empty guild is a real value and sorts first.
        assert_eq!(ascii_ci_cmp("", "Legacy"), Ordering::Less);
    }

    /// `"group"` is a live chain key with a dead arm: it displaces whatever was at the front, and
    /// orders nothing itself, so the list falls straight through to the rest of the chain.
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

    /// Every key ties ⇒ `Equal`, the comparator's `xor eax,eax` tail. Only reachable for two rows
    /// that agree on all six live keys.
    #[test]
    fn identical_rows_tie_on_every_key() {
        let chain = WhoSortChain::default();
        let a = row("Same", 30, "Priest", "Ironforge");
        assert_eq!(chain.compare(&a, &a.clone()), Ordering::Equal);
    }
}
