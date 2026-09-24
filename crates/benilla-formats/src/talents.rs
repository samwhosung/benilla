//! `Talent.dbc` and `TalentTab.dbc`: each class's three talent tabs, and each tab's talents with
//! grid seat, rank spells and prerequisites; `CMSG_LEARN_TALENT` carries a row id from here. The
//! columns are vmangos's `TalentEntry` and `TalentTabEntry` (`DBCStructure.h`).
//!
//! Order matters: `GetTalentInfo(tab, index)` indexes a tab's talents in native row order
//! (`TalentTabInfo[+0xC] + (index−1)*0x54`), never a (tier, column) sort, and a class's tabs come
//! in file order among rows matching both masks (`0x4f2e50`), never by `orderIndex` (column 13).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const TALENT: &str = "DBFilesClient\\Talent.dbc";
const TALENT_TAB: &str = "DBFilesClient\\TalentTab.dbc";

const TALENT_FIELDS: usize = 21;
const COL_T_TAB: usize = 1;
const COL_T_ROW: usize = 2;
const COL_T_COL: usize = 3;
const COL_T_RANK0: usize = 4; // ..=8, five rank spells
const COL_T_PREREQ: usize = 13;
const COL_T_PREREQ_RANK: usize = 16;
const COL_T_FLAGS: usize = 19;
const COL_T_REQUIRED_SPELL: usize = 20;

const TALENT_TAB_FIELDS: usize = 15;
const COL_TT_NAME_ENUS: usize = 1;
const COL_TT_RACE_MASK: usize = 11;
const COL_TT_CLASS_MASK: usize = 12;
const COL_TT_BACKGROUND: usize = 14;

/// The vanilla rank ceiling (`MAX_TALENT_RANK`, vmangos `DBCStructure.h`).
pub const MAX_TALENT_RANK: usize = 5;

/// One `Talent.dbc` row: a talent's grid seat, rank spells and prerequisites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Talent {
    pub id: u32,
    /// The owning `TalentTab.dbc` id.
    pub tab: u32,
    /// Tier, 0-based (the reference grid draws it 1-based).
    pub row: u32,
    /// Column, 0-based (grid width is 4).
    pub col: u32,
    /// The spell taught at each rank; a talent with `n` ranks fills exactly the first `n`.
    pub ranks: [u32; MAX_TALENT_RANK],
    /// Prerequisite talent id (0 for none) and the 0-based rank it needs. Every shipped
    /// prerequisite is in the same tab, the only arrows the reference frame draws.
    pub prereq_talent: u32,
    pub prereq_rank: u32,
    /// A spell that must be known regardless of talents (0 = none).
    pub required_spell: u32,
    /// Flags bit 0 (`TalentRec+0x4c`), the Lua API's `isExceptional`: an activatable ability.
    pub exceptional: bool,
}

impl Talent {
    /// How many ranks this talent has, the count of populated rank spells.
    pub fn max_rank(&self) -> u32 {
        self.ranks.iter().take_while(|&&s| s != 0).count() as u32
    }
}

/// One `TalentTab.dbc` row, a class page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TalentTabInfo {
    pub id: u32,
    pub name: String,
    /// `raceMask` and `classMask`, bit `1 << (id − 1)`; the client admits a tab only when both
    /// match (511 is every race in the shipped data).
    pub race_mask: u32,
    pub class_mask: u32,
    /// The `Interface\TalentFrame\<background>-…` art base ("MageFire").
    pub background: String,
}

/// Talent tabs by class and talents by tab.
pub struct TalentCatalog {
    tabs: Vec<TalentTabInfo>,
    /// Tab id to its talents in native row order, the order `GetTalentInfo` indexes.
    by_tab: HashMap<u32, Vec<Talent>>,
}

impl TalentCatalog {
    /// A race and class's tabs (unit bytes, 1-based) in file order, as `0x4f2e50` filters them.
    pub fn tabs_for_class(&self, race: u8, class: u8) -> Vec<&TalentTabInfo> {
        if !(1..=32).contains(&class) || !(1..=32).contains(&race) {
            return Vec::new();
        }
        let race_bit = 1u32 << (race - 1);
        let class_bit = 1u32 << (class - 1);
        self.tabs
            .iter()
            .filter(|t| t.class_mask & class_bit != 0 && t.race_mask & race_bit != 0)
            .collect()
    }

    /// A tab's talents in native row order, the display and index order.
    pub fn talents_in_tab(&self, tab_id: u32) -> &[Talent] {
        self.by_tab.get(&tab_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// One talent by id, for resolving a prerequisite arrow.
    pub fn talent(&self, tab_id: u32, talent_id: u32) -> Option<&Talent> {
        self.talents_in_tab(tab_id)
            .iter()
            .find(|t| t.id == talent_id)
    }

    pub fn len(&self) -> usize {
        self.by_tab.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tab.is_empty()
    }
}

fn talent_schema() -> Schema {
    let mut s = Schema::new("Talent");
    for i in 0..TALENT_FIELDS {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

fn talent_tab_schema() -> Schema {
    let mut s = Schema::new("TalentTab");
    for i in 0..TALENT_TAB_FIELDS {
        if i == COL_TT_NAME_ENUS {
            s.add_field(SchemaField::new("NameEnUs", FieldType::String));
        } else if i == COL_TT_BACKGROUND {
            s.add_field(SchemaField::new("BackgroundFile", FieldType::String));
        } else {
            s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
        }
    }
    s
}

/// Load both tables off the patch chain.
pub fn load_talent_catalog(chain: &mut Chain) -> Result<TalentCatalog> {
    let tab_bytes = chain
        .read_file(TALENT_TAB)
        .context("reading TalentTab.dbc")?;
    let tab_set = parse(&tab_bytes, talent_tab_schema(), "TalentTab.dbc")?;
    let mut tabs = Vec::new();
    for r in tab_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        tabs.push(TalentTabInfo {
            id,
            name: str_at(&tab_set, r, COL_TT_NAME_ENUS).unwrap_or_default(),
            race_mask: u32_at(r, COL_TT_RACE_MASK).unwrap_or(0),
            class_mask: u32_at(r, COL_TT_CLASS_MASK).unwrap_or(0),
            background: str_at(&tab_set, r, COL_TT_BACKGROUND).unwrap_or_default(),
        });
    }

    let t_bytes = chain.read_file(TALENT).context("reading Talent.dbc")?;
    let t_set = parse(&t_bytes, talent_schema(), "Talent.dbc")?;
    let mut by_tab: HashMap<u32, Vec<Talent>> = HashMap::new();
    for r in t_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut ranks = [0u32; MAX_TALENT_RANK];
        for (i, rank) in ranks.iter_mut().enumerate() {
            *rank = u32_at(r, COL_T_RANK0 + i).unwrap_or(0);
        }
        let t = Talent {
            id,
            tab: u32_at(r, COL_T_TAB).unwrap_or(0),
            row: u32_at(r, COL_T_ROW).unwrap_or(0),
            col: u32_at(r, COL_T_COL).unwrap_or(0),
            ranks,
            prereq_talent: u32_at(r, COL_T_PREREQ).unwrap_or(0),
            prereq_rank: u32_at(r, COL_T_PREREQ_RANK).unwrap_or(0),
            required_spell: u32_at(r, COL_T_REQUIRED_SPELL).unwrap_or(0),
            exceptional: u32_at(r, COL_T_FLAGS).unwrap_or(0) & 1 != 0,
        };
        // Native row order, never sorted.
        by_tab.entry(t.tab).or_default().push(t);
    }

    Ok(TalentCatalog { tabs, by_tab })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_talent_tables_hold_the_grid_and_prereq_invariants() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_talent_catalog(&mut chain).expect("load Talent/TalentTab");
        assert!(
            cat.len() > 300,
            "the 1.12 talent table has hundreds of rows, got {}",
            cat.len()
        );

        const HUMAN: u8 = 1;
        const MAGE: u8 = 8;
        let tabs = cat.tabs_for_class(HUMAN, MAGE);
        assert_eq!(
            tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["Arcane", "Fire", "Frost"]
        );
        assert_eq!(
            tabs.iter()
                .map(|t| t.background.as_str())
                .collect::<Vec<_>>(),
            vec!["MageArcane", "MageFire", "MageFrost"]
        );
        // Every shipped tab's raceMask is 511, so race 10 admits nothing.
        assert!(
            cat.tabs_for_class(10, MAGE).is_empty(),
            "race bit 10 is outside 511"
        );
        // Every class has pages; vanilla's classes are 1-5, 7-9 and 11 (druid).
        for class in [1u8, 2, 3, 4, 5, 7, 8, 9, 11] {
            let n = cat.tabs_for_class(HUMAN, class).len();
            assert_eq!(n, 3, "class {class} has three talent pages, got {n}");
        }

        for tabs in cat.tabs_for_class(HUMAN, MAGE) {
            let talents = cat.talents_in_tab(tabs.id);
            assert!(!talents.is_empty());
            let seats: Vec<(u32, u32)> = talents.iter().map(|t| (t.row, t.col)).collect();
            let mut sorted = seats.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                seats, sorted,
                "the shipped data's (row, col)-ordered authoring"
            );
        }

        let all_ids: std::collections::HashSet<u32> =
            cat.by_tab.values().flatten().map(|t| t.id).collect();
        for t in cat.by_tab.values().flatten() {
            assert!(
                t.col < 4,
                "talent {} col {} inside the 4-wide grid",
                t.id,
                t.col
            );
            assert!(t.row < 8, "talent {} row {} inside 8 tiers", t.id, t.row);
            let n = t.max_rank() as usize;
            assert!(n >= 1, "talent {} has at least one rank", t.id);
            assert!(
                t.ranks[n..].iter().all(|&s| s == 0),
                "talent {} rank array is prefix-shaped",
                t.id
            );
            if t.prereq_talent != 0 {
                assert!(
                    all_ids.contains(&t.prereq_talent),
                    "talent {} prereq {} resolves",
                    t.id,
                    t.prereq_talent
                );
                assert!(
                    cat.talent(t.tab, t.prereq_talent).is_some(),
                    "talent {} prereq {} lives in the same tab",
                    t.id,
                    t.prereq_talent
                );
                assert!(
                    (t.prereq_rank as usize) < MAX_TALENT_RANK,
                    "talent {} prereq rank {} is 0-based sane",
                    t.id,
                    t.prereq_rank
                );
            }
        }
    }
}
