//! The skill tables: a spell's skill line, a line's name and icon, and the per-race/class routing
//! the spellbook tabs and the skills pane read. Columns follow vmangos `DBCfmt.h`/`DBCStructure.h`,
//! which parse the same 5875 files.
//!
//! - `SkillLine.dbc`, `SkillLinefmt = "nixssssssssxxxxxxxxxxi"`.
//! - `SkillLineAbility.dbc`, `SkillLineAbilityfmt = "niiiixxiiiiixxi"`: one row per spell, the
//!   first in file order, except `forward_spellid`, the first non-zero across the spell's rows as
//!   in vmangos's `SpellMgr::GetSpellBookSuccessorSpellId`.
//! - `SkillRaceClassInfo.dbc`, `SkillRaceClassInfofmt = "diiiiiix"`: the row admitting the
//!   player's race and class, which the reference resolves at `0x6ddf90`; here the first such row.
//!   With flag `0x80`, or with no such row, a spell's tab is General.
//! - `SkillLineCategory.dbc`, 11 fields read off the raw file: the skills-pane headers in
//!   `displayOrder`. `Not Displayed` (12) is an ordinary header; the reference hides
//!   `GENERIC (DND)` by `SkillRaceClassInfo.flags & 0x2`, not by category.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const SKILL_LINE: &str = "DBFilesClient\\SkillLine.dbc";
const SKILL_LINE_ABILITY: &str = "DBFilesClient\\SkillLineAbility.dbc";
const SKILL_RACE_CLASS_INFO: &str = "DBFilesClient\\SkillRaceClassInfo.dbc";

const SKILL_LINE_FIELDS: usize = 22;
const COL_SL_NAME_ENUS: usize = 3;
const COL_SL_DESC_ENUS: usize = 12;
const COL_SL_SPELL_ICON: usize = 21;

const SKILL_LINE_ABILITY_FIELDS: usize = 15;
const COL_SLA_SKILL_ID: usize = 1;
const COL_SLA_SPELL_ID: usize = 2;
const COL_SLA_REQ_SKILL_VALUE: usize = 7;
const COL_SLA_FORWARD_SPELL: usize = 8;
const COL_SLA_TRIVIAL_HIGH: usize = 10;
const COL_SLA_TRIVIAL_LOW: usize = 11;

/// Cap on a rank-chain walk; the 5875 data is acyclic, 9 hops at most, so only a bad DBC hits it.
const MAX_RANK_CHAIN: usize = 16;

const SKILL_LINE_CATEGORY: &str = "DBFilesClient\\SkillLineCategory.dbc";
const SKILL_LINE_CATEGORY_FIELDS: usize = 11;
const COL_SLC_NAME_ENUS: usize = 1;
const COL_SLC_ORDER: usize = 10;

const SKILL_RACE_CLASS_INFO_FIELDS: usize = 8;
const COL_SRCI_SKILL_ID: usize = 1;
const COL_SRCI_RACE_MASK: usize = 2;
const COL_SRCI_CLASS_MASK: usize = 3;
const COL_SRCI_FLAGS: usize = 4;
const COL_SRCI_MIN_LEVEL: usize = 5;
const COL_SRCI_COST_INDEX: usize = 7;

/// Flag `0x80` (cmangos `SKILL_FLAG_DISPLAY_SORTED`), which the tab classifier reads as the low
/// byte's sign: the line's spells go to the General tab.
const SKILL_FLAG_DISPLAY_SORTED: u32 = 0x80;

/// Flag `0x20` (vmangos `SKILL_FLAG_UNLEARNABLE`), the unlearn button's gate: the server drops and
/// flags a `CMSG_UNLEARN_SKILL` for a line without it (`SkillHandler.cpp`).
const SKILL_FLAG_UNLEARNABLE: u32 = 0x20;

/// Flag `0x1`: the Skills tab lists the line even at rank 0 (`0x4d2cb0`); unnamed in mangos.
const SKILL_FLAG_ALWAYS_DISPLAY: u32 = 0x1;

/// Flag `0x2`: the Skills tab drops the line (`0x4d2d9f`); it also silences skill-ups, hence
/// mangos's name, `SKILL_FLAG_NO_SKILLUP_MESSAGE`.
const SKILL_FLAG_HIDDEN: u32 = 0x2;

/// Flag `0x4`: an untrained line shows from the row's `reqLevel`; unnamed in mangos.
const SKILL_FLAG_TRAINABLE_AT_LEVEL: u32 = 0x4;

/// Flag `0x400` (vmangos `SKILL_FLAG_MONO_VALUE`): `GetSkillLineInfo` returns `skillMaxRank` 1
/// whatever the descriptor says (`0x4d3610`), so `SkillFrame.lua` draws a proficiency bar.
const SKILL_FLAG_MONO_VALUE: u32 = 0x400;

/// A skill line's display: the spellbook tab's name and icon, and its skills-pane category.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillLineInfo {
    pub name: String,
    /// `categoryId`, column 1: the skills-pane group ([`SkillLineCatalog::category`]).
    pub category_id: u32,
    /// The tab icon's extensionless path (`Interface\Icons\…`), from `spellIcon`, column 21.
    pub icon: Option<String>,
    /// enUS `description_lang`, column 12: `GetSkillLineInfo`'s 13th return.
    pub description: String,
}

/// One `SkillRaceClassInfo.dbc` row: the race and class masks, and its [`SkillRaceClass`].
#[derive(Clone, Copy, Debug)]
struct SrciRow {
    race_mask: u32,
    class_mask: u32,
    row: SkillRaceClass,
}

/// The `SkillRaceClassInfo.dbc` fields the reference's Skills tab reads for a line, race and class
/// (the list build `0x4d2cb0`, `GetSkillLineInfo` `0x4d3610`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillRaceClass {
    /// `flags`, column 4.
    pub flags: u32,
    /// `reqLevel`, column 5: the level an untrained `0x4` line starts showing at.
    pub min_level: u32,
    /// `skillCostID`, column 7: `GetSkillLineInfo`'s 12th return is this plus one (`0x4d3a06`).
    pub cost_index: u32,
}

impl SkillRaceClass {
    /// Whether the Skills tab drops the line: this keeps `Dual Wield`, the racials, the riding
    /// lines and `GENERIC (DND)` off the pane though the server sends them like any other.
    pub fn hidden(self) -> bool {
        self.flags & SKILL_FLAG_HIDDEN != 0
    }

    /// Whether the line is single-rank ([`SKILL_FLAG_MONO_VALUE`]).
    pub fn mono(self) -> bool {
        self.flags & SKILL_FLAG_MONO_VALUE != 0
    }

    /// Whether a rank change prints no skill-up line: the reference's watcher (`0x5de180`) skips it
    /// when the flags carry `0x402`, tested at `0x5de358` on the row `0x6ddf90` resolves, never on
    /// a `SkillLine.dbc` field.
    pub fn skill_up_silent(self) -> bool {
        self.flags & (SKILL_FLAG_MONO_VALUE | SKILL_FLAG_HIDDEN) != 0
    }

    /// Whether the line can be unlearned; the reference also requires a nonzero skill step, which
    /// the descriptor carries.
    pub fn unlearnable(self) -> bool {
        self.flags & SKILL_FLAG_UNLEARNABLE != 0
    }

    /// Whether a rank-0 line gets a row at `player_level` (`0x4d2cb0`); a trained line never asks.
    pub fn displays_untrained(self, player_level: u32) -> bool {
        self.flags & SKILL_FLAG_ALWAYS_DISPLAY != 0
            || (self.flags & SKILL_FLAG_TRAINABLE_AT_LEVEL != 0 && player_level >= self.min_level)
    }
}

/// A spell's `SkillLineAbility.dbc` row: its line, required rank, and the trivial ranks the
/// crafting book's difficulty colours band on (`0x4fca20`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlaInfo {
    /// `skillId`, column 1.
    pub skill_id: u32,
    /// `req_skill_value`, column 7: the line rank the ability needs.
    pub req_skill_value: u32,
    /// `forward_spellid`, column 8: the next rank, or 0. Only the abilities the server supersedes
    /// carry it (vmangos `Player::AddSpell`); caster ranks carry 0 and all stay castable.
    pub forward_spell_id: u32,
    /// `min_value`, column 11: yellow from here, orange below.
    pub trivial_low: u32,
    /// `max_value`, column 10: gray from here, green from the low/high midpoint; 0 on non-recipes.
    pub trivial_high: u32,
}

/// The skill tables joined: a spell's line, a line's display, and the per-race/class routing.
pub struct SkillLineCatalog {
    lines: HashMap<u32, SkillLineInfo>,
    abilities: HashMap<u32, SlaInfo>,
    /// `SkillLineCategory.dbc`: id to (enUS name, displayOrder).
    categories: HashMap<u32, (String, u32)>,
    /// Skill line id to its `SkillRaceClassInfo` rows; empty if the DBC failed to load.
    race_class: HashMap<u32, Vec<SrciRow>>,
    /// `forward_spellid` inverted, next rank to previous; injective on the 5875 data.
    rank_prev: HashMap<u32, u32>,
}

impl SkillLineCatalog {
    /// A catalog of spell-to-line pairs only, for tests and fixtures.
    pub fn from_spell_lines(pairs: impl IntoIterator<Item = (u32, u32)>) -> Self {
        Self {
            abilities: pairs
                .into_iter()
                .map(|(spell, line)| {
                    (
                        spell,
                        SlaInfo {
                            skill_id: line,
                            req_skill_value: 0,
                            forward_spell_id: 0,
                            trivial_low: 0,
                            trivial_high: 0,
                        },
                    )
                })
                .collect(),
            lines: HashMap::new(),
            categories: HashMap::new(),
            race_class: HashMap::new(),
            rank_prev: HashMap::new(),
        }
    }

    /// The skill line a spell belongs to, if `SkillLineAbility.dbc` names one.
    pub fn spell_to_line(&self, spell_id: u32) -> Option<u32> {
        self.abilities.get(&spell_id).map(|a| a.skill_id)
    }

    /// A spell's `SkillLineAbility` row.
    pub fn ability(&self, spell_id: u32) -> Option<&SlaInfo> {
        self.abilities.get(&spell_id)
    }

    /// The next rank of `spell_id`, as vmangos's `SpellMgr::GetSpellBookSuccessorSpellId`.
    pub fn rank_successor(&self, spell_id: u32) -> Option<u32> {
        self.abilities
            .get(&spell_id)
            .map(|a| a.forward_spell_id)
            .filter(|&id| id != 0)
    }

    /// The first rank of `spell_id`'s chain, `spell_id` itself when it has none.
    fn chain_head(&self, spell_id: u32) -> u32 {
        let mut head = spell_id;
        for _ in 0..MAX_RANK_CHAIN {
            match self.rank_prev.get(&head) {
                Some(&prev) if prev != head => head = prev,
                _ => break,
            }
        }
        head
    }

    /// Whether a higher rank of `spell_id` is known, the reference's `KnownHigherRank`
    /// (`0x60c8d0`); the trainer's colouring and state re-evaluator OR it with plain known-ness.
    pub fn higher_rank_known(
        &self,
        spell_id: u32,
        known: &std::collections::BTreeSet<u32>,
    ) -> bool {
        let mut cur = spell_id;
        for _ in 0..MAX_RANK_CHAIN {
            let Some(next) = self.rank_successor(cur) else {
                return false;
            };
            if known.contains(&next) {
                return true;
            }
            cur = next;
        }
        false
    }

    /// A catalog of ability rows only, for tests.
    pub fn from_abilities(abilities: impl IntoIterator<Item = (u32, SlaInfo)>) -> Self {
        let abilities: HashMap<u32, SlaInfo> = abilities.into_iter().collect();
        let rank_prev = abilities
            .iter()
            .filter(|(_, a)| a.forward_spell_id != 0)
            .map(|(id, a)| (a.forward_spell_id, *id))
            .collect();
        Self {
            abilities,
            lines: HashMap::new(),
            categories: HashMap::new(),
            race_class: HashMap::new(),
            rank_prev,
        }
    }

    /// The highest known rank of `spell_id`'s chain, the rank an action-bar slot must hold. It
    /// walks from the chain's head, so a downgrade resolves too; the server keeps one rank of a
    /// chain active (vmangos `Player::AddSpell`).
    pub fn highest_known_rank(
        &self,
        spell_id: u32,
        known: &std::collections::BTreeSet<u32>,
    ) -> Option<u32> {
        let mut cur = self.chain_head(spell_id);
        let mut best = known.contains(&cur).then_some(cur);
        for _ in 0..MAX_RANK_CHAIN {
            let Some(next) = self.rank_successor(cur) else {
                break;
            };
            cur = next;
            if known.contains(&cur) {
                best = Some(cur);
            }
        }
        best
    }

    /// The spellbook tab for a spell and a 1-based `race`/`class`: its skill line, or 0 for
    /// General. An unknown race or class, or no routing data, keeps the raw line.
    pub fn spell_tab(&self, spell_id: u32, race: u8, class: u8) -> u32 {
        let Some(line) = self.spell_to_line(spell_id) else {
            return 0; // no skill line → General
        };
        if self.race_class.is_empty() || !(1..=32).contains(&race) || !(1..=32).contains(&class) {
            return line;
        }
        match self.srci_row(line, race, class) {
            Some(r) if r.row.flags & SKILL_FLAG_DISPLAY_SORTED == 0 => line,
            _ => 0,
        }
    }

    /// The first row of `line_id` admitting `race`/`class`; a zero mask admits all.
    fn srci_row(&self, line_id: u32, race: u8, class: u8) -> Option<&SrciRow> {
        if !(1..=32).contains(&race) || !(1..=32).contains(&class) {
            return None;
        }
        let race_bit = 1u32 << (race - 1);
        let class_bit = 1u32 << (class - 1);
        self.race_class.get(&line_id).and_then(|rows| {
            rows.iter().find(|r| {
                (r.race_mask == 0 || r.race_mask & race_bit != 0)
                    && (r.class_mask == 0 || r.class_mask & class_bit != 0)
            })
        })
    }

    /// Whether `race`/`class` can unlearn `line_id`, the server's own gate (`SkillHandler.cpp`);
    /// false when unresolved, as the server flags a refused unlearn.
    pub fn abandonable(&self, line_id: u32, race: u8, class: u8) -> bool {
        self.race_class(line_id, race, class)
            .is_some_and(SkillRaceClass::unlearnable)
    }

    /// The row the reference resolves for `line_id`, `race` and `class`; with none, its list build
    /// drops the line (`0x4d2cb0`), and so must the pane.
    pub fn race_class(&self, line_id: u32, race: u8, class: u8) -> Option<SkillRaceClass> {
        self.srci_row(line_id, race, class).map(|r| r.row)
    }

    /// Whether `line_id` is single-rank for `race`/`class`; unresolved keeps the server's numbers.
    pub fn mono_value(&self, line_id: u32, race: u8, class: u8) -> bool {
        self.race_class(line_id, race, class)
            .is_some_and(SkillRaceClass::mono)
    }

    /// Whether a rank change in `line_id` prints a skill-up line for `race`/`class`; with no
    /// admitting row the reference stays silent too (`0x5de352`).
    pub fn announces_skill_ups(&self, line_id: u32, race: u8, class: u8) -> bool {
        self.race_class(line_id, race, class)
            .is_some_and(|rc| !rc.skill_up_silent())
    }

    /// A skill line's display, by id.
    pub fn line(&self, line_id: u32) -> Option<&SkillLineInfo> {
        self.lines.get(&line_id)
    }

    /// A `SkillLineCategory.dbc` row's `(name, displayOrder)`, a skills-pane header.
    pub fn category(&self, category_id: u32) -> Option<(&str, u32)> {
        self.categories
            .get(&category_id)
            .map(|(n, o)| (n.as_str(), *o))
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

fn skill_line_schema() -> Schema {
    let mut s = Schema::new("SkillLine");
    for i in 0..SKILL_LINE_FIELDS {
        if i == COL_SL_NAME_ENUS {
            s.add_field(SchemaField::new("NameEnUs", FieldType::String));
        } else if i == COL_SL_DESC_ENUS {
            s.add_field(SchemaField::new("DescEnUs", FieldType::String));
        } else {
            s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
        }
    }
    s
}

fn skill_line_ability_schema() -> Schema {
    let mut s = Schema::new("SkillLineAbility");
    for i in 0..SKILL_LINE_ABILITY_FIELDS {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

fn skill_line_category_schema() -> Schema {
    let mut s = Schema::new("SkillLineCategory");
    for i in 0..SKILL_LINE_CATEGORY_FIELDS {
        let ty = if i == COL_SLC_NAME_ENUS {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// `SkillLineCategory.dbc`; an unreadable file gives an empty map and the pane one flat group.
fn load_categories(chain: &mut Chain) -> HashMap<u32, (String, u32)> {
    let mut map = HashMap::new();
    let Ok(bytes) = chain.read_file(SKILL_LINE_CATEGORY) else {
        return map;
    };
    let Ok(set) = parse(
        &bytes,
        skill_line_category_schema(),
        "SkillLineCategory.dbc",
    ) else {
        return map;
    };
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(name) = str_at(&set, r, COL_SLC_NAME_ENUS) {
            map.insert(id, (name, u32_at(r, COL_SLC_ORDER).unwrap_or(0)));
        }
    }
    map
}

fn skill_race_class_info_schema() -> Schema {
    let mut s = Schema::new("SkillRaceClassInfo");
    for i in 0..SKILL_RACE_CLASS_INFO_FIELDS {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

/// `SkillRaceClassInfo.dbc` rows by skill line; an unreadable file gives an empty map.
fn load_race_class_info(chain: &mut Chain) -> HashMap<u32, Vec<SrciRow>> {
    let mut map: HashMap<u32, Vec<SrciRow>> = HashMap::new();
    let bytes = match chain.read_file(SKILL_RACE_CLASS_INFO) {
        Ok(b) => b,
        Err(_) => return map,
    };
    let set = match parse(
        &bytes,
        skill_race_class_info_schema(),
        "SkillRaceClassInfo.dbc",
    ) {
        Ok(s) => s,
        Err(_) => return map,
    };
    for r in set.records() {
        let Some(skill) = u32_at(r, COL_SRCI_SKILL_ID) else {
            continue;
        };
        map.entry(skill).or_default().push(SrciRow {
            race_mask: u32_at(r, COL_SRCI_RACE_MASK).unwrap_or(0),
            class_mask: u32_at(r, COL_SRCI_CLASS_MASK).unwrap_or(0),
            row: SkillRaceClass {
                flags: u32_at(r, COL_SRCI_FLAGS).unwrap_or(0),
                min_level: u32_at(r, COL_SRCI_MIN_LEVEL).unwrap_or(0),
                cost_index: u32_at(r, COL_SRCI_COST_INDEX).unwrap_or(0),
            },
        });
    }
    map
}

/// Load the joined skill-line catalog off the patch chain.
pub fn load_skill_line_catalog(chain: &mut Chain) -> Result<SkillLineCatalog> {
    let icons = crate::dbc::load_spell_icon_map(chain)?;

    let sl_bytes = chain
        .read_file(SKILL_LINE)
        .context("reading SkillLine.dbc")?;
    let sl_set = parse(&sl_bytes, skill_line_schema(), "SkillLine.dbc")?;
    let mut lines: HashMap<u32, SkillLineInfo> = HashMap::new();
    for r in sl_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let name = str_at(&sl_set, r, COL_SL_NAME_ENUS).unwrap_or_default();
        let icon = u32_at(r, COL_SL_SPELL_ICON)
            .filter(|&i| i != 0)
            .and_then(|i| icons.get(&i).cloned());
        let category_id = u32_at(r, 1).unwrap_or(0);
        let description = str_at(&sl_set, r, COL_SL_DESC_ENUS).unwrap_or_default();
        lines.insert(
            id,
            SkillLineInfo {
                name,
                category_id,
                icon,
                description,
            },
        );
    }

    let sla_bytes = chain
        .read_file(SKILL_LINE_ABILITY)
        .context("reading SkillLineAbility.dbc")?;
    let sla_set = parse(
        &sla_bytes,
        skill_line_ability_schema(),
        "SkillLineAbility.dbc",
    )?;
    let mut abilities: HashMap<u32, SlaInfo> = HashMap::new();
    for r in sla_set.records() {
        if let (Some(skill_id), Some(spell_id)) =
            (u32_at(r, COL_SLA_SKILL_ID), u32_at(r, COL_SLA_SPELL_ID))
        {
            let forward_spell_id = u32_at(r, COL_SLA_FORWARD_SPELL).unwrap_or(0);
            // The first row in file order wins.
            let slot = abilities.entry(spell_id).or_insert(SlaInfo {
                skill_id,
                req_skill_value: u32_at(r, COL_SLA_REQ_SKILL_VALUE).unwrap_or(0),
                forward_spell_id,
                trivial_low: u32_at(r, COL_SLA_TRIVIAL_LOW).unwrap_or(0),
                trivial_high: u32_at(r, COL_SLA_TRIVIAL_HIGH).unwrap_or(0),
            });
            // Except the rank link: the first non-zero across the rows, as vmangos scans them all.
            if slot.forward_spell_id == 0 {
                slot.forward_spell_id = forward_spell_id;
            }
        }
    }
    let rank_prev = abilities
        .iter()
        .filter(|(_, a)| a.forward_spell_id != 0)
        .map(|(&spell, a)| (a.forward_spell_id, spell))
        .collect();

    let race_class = load_race_class_info(chain);
    let categories = load_categories(chain);

    Ok(SkillLineCatalog {
        lines,
        abilities,
        categories,
        race_class,
        rank_prev,
    })
}

#[cfg(test)]
mod tests;
