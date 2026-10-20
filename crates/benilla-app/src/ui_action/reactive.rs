//! The reactive spells behind `SPELL_ACTIVE`: the list `[0xb71150]` (count `[0xb7114c]`) of known
//! spells with `AttributesEx4 & 0x200`, which the usable refresh `0x4b31c0` re-checks after the bar
//! (`0x4b31d7`-`0x4b323d`), raising `COMBAT_TEXT_UPDATE` `SPELL_ACTIVE` with the spell's name for
//! each that turns usable (`0x4b320b`-`0x4b3226`), whether or not a bar holds it.

use std::collections::BTreeSet;

use benilla_formats::{SpellCatalog, SpellDisplay};

/// `AttributesEx4` bit 9 (`SpellRec+0x28`), which enters a learned spell in the list
/// (`0x4b2702`-`0x4b270b`).
const ATTR_EX4_REACTIVE: u32 = 0x200;

/// One entry: the spell it checks and the usable answer it last stored (`0x4b3231`).
struct Entry {
    spell_id: u32,
    name: String,
    usable: bool,
}

/// The list, one entry per spell name, the highest rank (`0x4b2720`-`0x4b2844`), each answer 0 when
/// it enters (`0x4b27c7`). `0x4b23a0` empties it at the game's start (`0x48f59f` → `0x4b21c0`) and
/// at the active player's arrival (`0x5dea86`); a `/reload` (`0x495664`) reaches neither.
#[derive(Default)]
pub(super) struct ReactiveList {
    entries: Vec<Entry>,
}

/// One refresh's outcome.
#[derive(Default)]
pub(super) struct Refresh {
    /// Some entry's answer moved, either way: the refresh's `SPELL_UPDATE_USABLE` is due.
    pub(super) changed: bool,
    /// The names that turned usable, in list order, each a `SPELL_ACTIVE`.
    pub(super) activated: Vec<String>,
}

impl ReactiveList {
    /// Empty the list: `0x4b23a0` at the active player's arrival.
    pub(super) fn arrive(&mut self) {
        self.entries.clear();
    }

    /// Take in the book's reactive spells: a name the list lacks enters (`0x4b27b3`-`0x4b27c7`),
    /// and a held name moves to a strictly higher rank by `ParseSpellRank` (`0x4b3c30`, `0x4b2838`),
    /// keeping its answer (`0x4b2844`). Nothing leaves before the next arrival: unlearning
    /// (`0x4b2c50`) never touches the list.
    pub(super) fn sync(&mut self, book: &BTreeSet<u32>, catalog: &SpellCatalog) {
        for (id, d) in book
            .iter()
            .filter_map(|&id| catalog.get(id).map(|d| (id, d)))
            .filter(|(_, d)| d.attributes_ex4 & ATTR_EX4_REACTIVE != 0)
        {
            // `SStrCmp` (`0x64a480`) of the two names: case counts.
            match self.entries.iter_mut().find(|e| e.name == d.name) {
                Some(e) => {
                    if catalog
                        .get(e.spell_id)
                        .is_none_or(|held| parse_rank(d) > parse_rank(held))
                    {
                        e.spell_id = id;
                    }
                }
                None => self.entries.push(Entry {
                    spell_id: id,
                    name: d.name.clone(),
                    usable: false,
                }),
            }
        }
    }

    /// `0x4b31c0`'s loop: each entry's answer from `usable`, stored when it moved (`0x4b320b`,
    /// `0x4b3231`), and the names that turned usable.
    pub(super) fn refresh(&mut self, mut usable: impl FnMut(u32) -> bool) -> Refresh {
        let mut out = Refresh::default();
        for e in &mut self.entries {
            let now = usable(e.spell_id);
            if now == e.usable {
                continue;
            }
            out.changed = true;
            if now {
                out.activated.push(e.name.clone());
            }
            e.usable = now;
        }
        out
    }
}

/// `ParseSpellRank` (`0x4b3c30`): the rank string's digits read as a number, ending at the first
/// other character once the number is not 0 (`0x4b3c7e`), else the spell's `SpellLevel` (`+0x74`).
fn parse_rank(d: &SpellDisplay) -> u32 {
    let mut n: u32 = 0;
    for c in d.rank.as_deref().unwrap_or_default().bytes() {
        if c.is_ascii_digit() {
            // `lea`, `lea`: 32-bit and wrapping.
            n = n.wrapping_mul(10).wrapping_add(u32::from(c - b'0'));
        } else if n != 0 {
            return n;
        }
    }
    if n != 0 {
        n
    } else {
        d.spell_level
    }
}
