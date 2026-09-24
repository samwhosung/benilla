//! `SkillLine.dbc` + `SkillLineAbility.dbc` loader — spell id → skill line → {name, icon}, the
//! spellbook's tab source ("tabs = the class skill lines of known spells").
//!
//! Layout — VERIFIED against the **vmangos server source**
//! (`vmangos-src/src/game/Database/DBCStructure.h`'s `SkillLineEntry`/`SkillLineAbilityEntry`
//! structs + `DBCfmt.h`'s `SkillLinefmt`/`SkillLineAbilityfmt` format strings): vmangos parses
//! these two tables straight off the same build-5875 client data benilla reads, so its struct
//! layout — not an empirical guess, and with no byte derivation of these two tables in the
//! reference — is the strongest ground available, the same standing this codebase
//! already gives vmangos's wire handlers (decision 0216's own citations of `ItemHandler.cpp`/
//! `Player.cpp`).
//!
//! `SkillLine.dbc` — `SkillLinefmt = "nixssssssssxxxxxxxxxxi"` (22 fields, 88 B/record):
//! `id`(0, indexed) · `categoryId`(1, int32) · `skillCostID`(2, unused) · the 8-locale
//! `displayName_lang` block (3..10, enUS first ⇒ **NameEnUs = column 3**) + its flags word (11) ·
//! the 8-locale `description_lang` block (12..19; enUS **column 12** = the skills pane's
//! detail-pane body, `GetSkillLineInfo`'s 13th return) + its flags word (20) ·
//! **`spellIcon` = column 21** (a `SpellIcon.dbc` id — [`crate::dbc::load_spell_icon_map`], the
//! same table `spells.rs`'s action-bar catalog joins against).
//!
//! `SkillLineAbility.dbc` — `SkillLineAbilityfmt = "niiiixxiiiiixxi"` (15 fields, 60 B/record):
//! `id`(0, indexed) · **`skillId` = column 1** · **`spellId` = column 2** · `racemask`(3) ·
//! `classmask`(4) · `req_skill_value`(7; 5-6 unused: `racemaskNot`/`classmaskNot`, always 0 this
//! build) · `forward_spellid`(8) · `learnOnGetSkill`(9) · `max_value`(10) · `min_value`(11) ·
//! `reqtrainpoints`(14; 12-13 unused). Read into one [`SlaInfo`] per spell (a spell can carry
//! more than one row across race/class variants; the FIRST row wins, deterministic by file
//! order — [`SkillLineCatalog::spell_to_line`]'s long-standing convention), except
//! `forward_spellid`, which takes the first **non-zero** across the spell's rows to match
//! vmangos's own `SpellMgr::GetSpellBookSuccessorSpellId` exactly (identical on this build:
//! probed, 44 spells carry more than one row and not one disagrees on the column). `max_value`/
//! `min_value` are the recipe-difficulty trivial ranks (TrivialSkillLineRankHigh/Low): pinned on
//! the raw 5875 file this session — Bolt of Linen Cloth 2963 → (line 197, req 1, low 25,
//! high 50), Minor Healing Potion 2330 → (171, 1, 55, 95), which reproduces its known classic
//! orange 1 / yellow 55 / green 75 / gray 95 progression under the client's color law
//! (`0x4fca20`).
//!
//! `SkillRaceClassInfo.dbc` — `SkillRaceClassInfofmt = "diiiiiix"` (8 fields, 32 B/record):
//! `id`(0) · **`skillId` = column 1** · **`raceMask` = column 2** · **`classMask` = column 3** ·
//! **`flags` = column 4** · **`reqLevel` = column 5** · `skillTierId`(6) ·
//! **`skillCostID` = column 7**. This is
//! the table the client's spellbook tab classifier routes through: a spell's skill
//! line is looked up here for the player's race+class, and if the matching row's `flags` bit `0x80`
//! (`SKILL_FLAG_DISPLAY_SORTED`, cmangos `DBCEnums.h`) is set — or no row matches — the spell's tab
//! is **General** (key 0) instead of the line's own tab
//! (`0x6ddf90(skillLine, class, race) → variant`; `(int8)[variant+0x10] < 0 → key 0`;
//! `[variant+4]` = skillId, `[variant+0x10]` = flags — the struct offsets confirm the column
//! read). The `flags`/`raceMask`/`classMask` semantics follow
//! vmangos `DBCStructure.h`'s `SkillRaceClassInfoEntry`; the row-match (first row whose masks
//! admit the race/class) is the standard classic semantics, validated against the real build-5875
//! data by [`SkillLineCatalog::spell_tab`]'s tests.
//!
//! `SkillLineCategory.dbc` — byte-checked on the raw 5875 file (a struct-unpack dump: 8 records ×
//! 11 fields, 44 B/record): `id`(0) · the 8-locale `name` block (enUS ⇒ **column 1**) + flags(9) ·
//! **`displayOrder` = column 10** — the skills pane's header vocabulary and group order (decision
//! 0437 phase 4): Class Skills(7, order 2) · Professions(11, 3) · Secondary(9, 4) · Weapon(6, 5) ·
//! Armor(8, 6) · Languages(10, 7); `Attributes`(5, 1) never carries player rows, and
//! `Not Displayed`(12, 8) is a header like any other — **not** a hide bucket, whatever its name
//! suggests: the client drops `GENERIC (DND)` by its `SkillRaceClassInfo.flags & 0x2`, never by its
//! category. A skill line's own `categoryId` is `SkillLine.dbc` column 1 (the
//! `SkillLinefmt` layout above).
//!
//! Skill line ids are stable, well-known constants across the whole classic tool ecosystem
//! (vmangos `SharedDefines.h`'s `SkillType` enum, itself commented "Data from SpellLine.dbc (1.12.1
//! checked)") — Frost=6, Fire=8, … — cross-checked directly by this module's own real-data tests.

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

/// Hard stop on a rank-chain walk ([`SkillLineCatalog::highest_known_rank`]). The real build-5875
/// data is acyclic and its longest chain is **9** hops (Heroic Strike 78 → … → 25286, probed over
/// every one of the 406 chained spells), so this only ever fires on edited/corrupt DBCs — where a
/// wrong answer beats a hung frame.
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

/// `SkillRaceClassInfo.flags` bit `0x80` — cmangos `DBCEnums.h`'s `SKILL_FLAG_DISPLAY_SORTED`. The
/// spellbook tab classifier reads it as the low byte's sign (`(int8) < 0`): set ⇒ the skill line's
/// spells sort into the **General** tab rather than the line's own. Real
/// build-5875 data for a human warrior: set on `Racial - Human`, `GENERIC (DND)`, the proficiency/
/// language/riding lines; clear on the class combat lines (`Arms`/`Fury`/`Protection`).
const SKILL_FLAG_DISPLAY_SORTED: u32 = 0x80;

/// `SkillRaceClassInfo.flags` bit `0x20` — vmangos `DBCEnums.h`'s `SKILL_FLAG_UNLEARNABLE`
/// ("Skill can be unlearned"): the skills pane's unlearn-button gate, and the exact bit the
/// server's own `CMSG_UNLEARN_SKILL` handler enforces (vmangos `SkillHandler.cpp` — a request
/// for a line without it is dropped and anticheat-flagged, so the client must never offer it).
const SKILL_FLAG_UNLEARNABLE: u32 = 0x20;

/// `SkillRaceClassInfo.flags` bit `0x1` — a line the Skills tab lists even at **rank 0** (the list
/// build's `0x4d2cb0` untrained gate). Unnamed in the mangos enums; named here for what the bytes
/// do.
const SKILL_FLAG_ALWAYS_DISPLAY: u32 = 0x1;

/// `SkillRaceClassInfo.flags` bit `0x2` — the Skills tab **drops the line entirely**
/// (`4d2d9f test dl,0x2`). mangos names this bit
/// `SKILL_FLAG_NO_SKILLUP_MESSAGE` from a different call site; in the display list it is a hide
/// bit, and it is what keeps `Dual Wield`, the racial lines, the per-mount riding lines and
/// `GENERIC (DND)` off the real client's pane. mangos's "different call site" is the skill-up
/// message watcher — the bit is half of [`SkillRaceClass::skill_up_silent`]'s `0x402` mask.
const SKILL_FLAG_HIDDEN: u32 = 0x2;

/// `SkillRaceClassInfo.flags` bit `0x4` — an untrained (rank 0) line becomes visible once the
/// player reaches the row's `reqLevel`. Also one of the two bits gating the client's step-cost
/// lookup; unnamed in the mangos enums.
const SKILL_FLAG_TRAINABLE_AT_LEVEL: u32 = 0x4;

/// `SkillRaceClassInfo.flags` bit `0x400` — vmangos `DBCEnums.h`'s `SKILL_FLAG_MONO_VALUE` (a
/// single-rank line). The real client's `GetSkillLineInfo` **overrides** its `skillMaxRank` return
/// to `1` whenever the admitting row carries this bit, whatever the player's own skill descriptor
/// says (`0x4d3610`, the `4d38b1 test ah,0x4` branch). That override is why a class skill the
/// server reports as `300/300` draws as `SkillFrame.lua`'s gray, rank-text-less "proficiency"
/// bar in the real client — the Lua gate is `skillMaxRank == 1`, and the DBC, not the wire, is
/// what puts it there. Real build-5875
/// data for a night-elf hunter: set on `Beast Mastery`/`Marksmanship`/`Survival` (0x410), `Dual
/// Wield`/`Night Elf Racial`/the per-mount riding lines (0x492); clear on every weapon line, the
/// armor proficiencies, the languages, `Riding` and the professions.
const SKILL_FLAG_MONO_VALUE: u32 = 0x400;

/// One skill line's display identity (`SkillLine.dbc`) — a spellbook tab's name + icon, and the
/// skills pane's grouping key.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillLineInfo {
    pub name: String,
    /// `categoryId` (column 1) — the [`SkillLineCategory`](Self) bucket the skills pane groups
    /// this line under ([`SkillLineCatalog::category`]); 0 when absent.
    pub category_id: u32,
    /// The tab icon's MPQ path (`Interface\Icons\…`, extensionless); `None` when the line's
    /// `spellIcon` id is 0/unresolved (render the fallback question mark, the spell catalog's own
    /// convention).
    pub icon: Option<String>,
    /// `description_lang` enUS (column 12) — the skills pane's detail-pane body
    /// (`GetSkillLineInfo`'s 13th return). Professions carry the trade's flavor sentence, weapon
    /// lines the shared "Higher weapon skill increases your chance to hit."; empty when the row
    /// has none.
    pub description: String,
}

/// One `SkillRaceClassInfo.dbc` row: which race/class it admits, plus everything the client reads
/// off it ([`SkillRaceClass`], the caller-facing half).
#[derive(Clone, Copy, Debug)]
struct SrciRow {
    race_mask: u32,
    class_mask: u32,
    row: SkillRaceClass,
}

/// The `SkillRaceClassInfo.dbc` row the client resolved for a given skill line × race × class —
/// the whole of what its Skills-tab display law reads (the list build `0x4d2cb0` and
/// `GetSkillLineInfo 0x4d3610`). Copy-cheap; obtained from
/// [`SkillLineCatalog::race_class`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillRaceClass {
    /// `flags` (column 4) — the bit field the predicates below read.
    pub flags: u32,
    /// `reqLevel` (column 5) — the player level at which a not-yet-trained line starts showing
    /// (only consulted with [`SKILL_FLAG_TRAINABLE_AT_LEVEL`]).
    pub min_level: u32,
    /// `skillCostID` (column 7) — `GetSkillLineInfo`'s 12th return is this **plus one**
    /// (`0x4d3a06`). Inert on this build: every branch that would paint from it is repainted by
    /// the normal-skill/proficiency branches after it.
    pub cost_index: u32,
}

impl SkillRaceClass {
    /// Whether the Skills tab drops this line **entirely** — `flags & 0x2`, the client's own
    /// `4d2d9f test dl,0x2 / jne` in the list build. This is what makes `Dual Wield`, the racial
    /// lines, the per-mount riding lines and `GENERIC (DND)` invisible in the real client even
    /// though the server sends them like any other skill.
    pub fn hidden(self) -> bool {
        self.flags & SKILL_FLAG_HIDDEN != 0
    }

    /// Whether this is a **single-rank** line — `flags & 0x400`, which makes the client's
    /// `GetSkillLineInfo` report `skillMaxRank = 1` whatever the descriptor says
    /// ([`SKILL_FLAG_MONO_VALUE`]).
    pub fn mono(self) -> bool {
        self.flags & SKILL_FLAG_MONO_VALUE != 0
    }

    /// Whether a change to this line prints **no** skill-up chat line — the real client's
    /// rank-watcher gate (`0x5de180`): the message is skipped when the
    /// resolved row's flag word at `+0x10` carries `0x402` =
    /// [`SKILL_FLAG_MONO_VALUE`]`|`[`SKILL_FLAG_HIDDEN`]. On the real build-5875 data the mask
    /// cuts the table exactly along the historically attested line: every class spec line
    /// (`0x410`), the racials / `Dual Wield` / `GENERIC (DND)` / the per-mount riding lines
    /// (`0x492`) and `Fist Weapons` (`0x082`) are silent — which is why a real 1.12 ding
    /// announces no skill at all, the level-up movers being all flagged — while the other weapon
    /// lines, `Defense`, the armor proficiencies, professions/secondary, `Lockpicking`,
    /// `Poisons` and the languages (`0x080`/`0x0a0`) announce. The table identity is settled at
    /// the bytes (1309): the flag test `0x5de358` reads the **untouched return of the `0x6ddf90`
    /// SkillRaceClassInfo resolve**, never a `SkillLine.dbc` field.
    pub fn skill_up_silent(self) -> bool {
        self.flags & (SKILL_FLAG_MONO_VALUE | SKILL_FLAG_HIDDEN) != 0
    }

    /// Whether the line can be unlearned — `flags & 0x20` ([`SKILL_FLAG_UNLEARNABLE`]). The
    /// client ANDs this with a nonzero skill **step**; that half lives with the descriptor, at
    /// the feed.
    pub fn unlearnable(self) -> bool {
        self.flags & SKILL_FLAG_UNLEARNABLE != 0
    }

    /// Whether a line the player holds at **rank 0** still gets a row, for a player at
    /// `player_level` — the list build's own gate (`0x4d2cb0`): shown outright with
    /// [`SKILL_FLAG_ALWAYS_DISPLAY`], else only when it is [`SKILL_FLAG_TRAINABLE_AT_LEVEL`] and
    /// the player has reached [`Self::min_level`]. A line at rank ≥ 1 never consults this.
    pub fn displays_untrained(self, player_level: u32) -> bool {
        self.flags & SKILL_FLAG_ALWAYS_DISPLAY != 0
            || (self.flags & SKILL_FLAG_TRAINABLE_AT_LEVEL != 0 && player_level >= self.min_level)
    }
}

/// One spell's `SkillLineAbility.dbc` row (module doc columns; first row wins across race/class
/// variants): the skill line it belongs to, the rank required to learn it, and the trivial ranks
/// the crafting book's difficulty colors band against (the client's color law `0x4fca20`,
/// decision 0446).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlaInfo {
    /// `skillId` (column 1) — the owning skill line.
    pub skill_id: u32,
    /// `req_skill_value` (column 7) — the line rank required to learn/use the ability.
    pub req_skill_value: u32,
    /// `forward_spellid` (column 8) — the **next rank** of this ability, or 0 when it has none.
    /// This column is the whole rank-chain graph: 406 of the 4753 spells carry it, and they are
    /// exactly the abilities the server supersedes (vmangos gates every `SMSG_SUPERCEDED_SPELL`
    /// it sends on `GetSpellBookSuccessorSpellId`, `Player::AddSpell`) — the warrior/rogue
    /// physical lines, the profession tier openers, weapon skills. Caster nukes/heals carry 0:
    /// their ranks all stay known and castable, which is what makes vanilla down-ranking work.
    pub forward_spell_id: u32,
    /// `min_value` (column 11) — TrivialSkillLineRankLow: yellow at-or-above, orange below.
    pub trivial_low: u32,
    /// `max_value` (column 10) — TrivialSkillLineRankHigh: gray at-or-above; green at-or-above
    /// the low/high midpoint. Both 0 on non-recipe rows (class abilities, the openers).
    pub trivial_high: u32,
}

/// `SkillLine.dbc` × `SkillLineAbility.dbc` × `SkillRaceClassInfo.dbc`, joined: a spell's skill
/// line, a line's display, and the per-race/class tab routing (the General collapse).
pub struct SkillLineCatalog {
    lines: HashMap<u32, SkillLineInfo>,
    abilities: HashMap<u32, SlaInfo>,
    /// `SkillLineCategory.dbc`: id → (enUS name, displayOrder) — the skills pane's headers.
    categories: HashMap<u32, (String, u32)>,
    /// skill line id → its `SkillRaceClassInfo` rows (empty when the DBC failed to load — then
    /// [`Self::spell_tab`] skips the General collapse and keeps each line its own tab).
    race_class: HashMap<u32, Vec<SrciRow>>,
    /// The `forward_spellid` graph inverted: next rank → the rank before it. Injective on the
    /// real build-5875 data (probed: not one of the 406 chained spells is the successor of two
    /// different predecessors), so a spell has at most one previous rank and
    /// [`Self::chain_head`]'s walk is unambiguous.
    rank_prev: HashMap<u32, u32>,
}

impl SkillLineCatalog {
    /// Build a catalog holding only a spell→line map — for tests and synthetic fixtures
    /// ([`crate::SpellCatalog::from_displays`]'s sibling). The live path is
    /// [`load_skill_line_catalog`]; everything but [`Self::spell_to_line`]/[`Self::ability`]
    /// answers empty.
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

    /// A spell's full `SkillLineAbility` row ([`SlaInfo`]) — the crafting book's difficulty and
    /// requirement source.
    pub fn ability(&self, spell_id: u32) -> Option<&SlaInfo> {
        self.abilities.get(&spell_id)
    }

    /// The **next rank** of `spell_id` ([`SlaInfo::forward_spell_id`]), or `None` when the ability
    /// doesn't rank up this way — vmangos's `SpellMgr::GetSpellBookSuccessorSpellId`.
    pub fn rank_successor(&self, spell_id: u32) -> Option<u32> {
        self.abilities
            .get(&spell_id)
            .map(|a| a.forward_spell_id)
            .filter(|&id| id != 0)
    }

    /// The **first** rank of `spell_id`'s chain — walk [`Self::rank_prev`] back to the spell that
    /// nothing forwards to. `spell_id` itself when it heads its own chain (including the common
    /// case of no chain at all).
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

    /// **Is a higher rank of `spell_id` known?** — the reference's `KnownHigherRank`
    /// (`0x60c8d0`): walk the `forward_spellid` chain forward
    /// from `spell_id` (exclusive) and answer on the first known rank. The trainer's requirement
    /// colouring and its state re-evaluator both OR this with plain known-ness (2333).
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

    /// A catalog holding only ability rows — for tests that need a rank chain or a skill
    /// requirement without the DBCs ([`Self::from_spell_lines`]'s fuller sibling).
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

    /// The **highest rank of `spell_id`'s ability that `known` contains** — the rank an action-bar
    /// slot pointing at `spell_id` must actually hold. `None` when no rank of the chain is known
    /// (an empty book, or an ability the character never learned): the caller leaves the slot
    /// alone rather than pointing it somewhere arbitrary.
    ///
    /// Walks the whole chain from its head, not just forward from `spell_id`, so it answers for a
    /// *downgrade* (the bar holds rank 5, the book was pushed back to rank 4) as well as the
    /// ordinary rank-up. The server only ever keeps one rank of a chained ability active
    /// (vmangos `Player::AddSpell` marks the old one `active = false` and it drops out of
    /// `SMSG_INITIAL_SPELLS`), so in practice at most one rank is ever known and "highest" is
    /// simply "the one".
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

    /// The spellbook **tab** a spell lands in for a character of `race`/`class` (1-based unit
    /// bytes): the spell's skill line, unless that line routes to General. Returns
    /// `0` (the General tab) when the spell has no skill line, no `SkillRaceClassInfo` row admits
    /// this race/class, or the matching row carries [`SKILL_FLAG_DISPLAY_SORTED`]; the line's own
    /// id otherwise. With `race`/`class` `0` or out of range (unknown character), or when no
    /// `SkillRaceClassInfo` data loaded, the collapse is skipped — the raw skill line is returned.
    pub fn spell_tab(&self, spell_id: u32, race: u8, class: u8) -> u32 {
        let Some(line) = self.spell_to_line(spell_id) else {
            return 0; // no skill line → General
        };
        // No character context, or no routing data — keep the raw line (pre-collapse behavior).
        if self.race_class.is_empty() || !(1..=32).contains(&race) || !(1..=32).contains(&class) {
            return line;
        }
        match self.srci_row(line, race, class) {
            // A matching row without the sort flag keeps the line's own tab.
            Some(r) if r.row.flags & SKILL_FLAG_DISPLAY_SORTED == 0 => line,
            // The sort flag, or no admitting row for this race/class → General.
            _ => 0,
        }
    }

    /// The first `SkillRaceClassInfo` row of `line_id` admitting a 1-based `race`/`class` (mask
    /// `0` admits all) — the standard classic row-match ([`Self::spell_tab`]'s own, factored out
    /// for [`Self::abandonable`]). `None` for out-of-range race/class or no admitting row.
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

    /// Whether `line_id` can be unlearned by a character of `race`/`class` (1-based unit bytes):
    /// the admitting `SkillRaceClassInfo` row carries [`SKILL_FLAG_UNLEARNABLE`] (`0x20`) — the
    /// skills pane's unlearn-button predicate, and byte-for-byte the server's own gate (vmangos
    /// `SkillHandler.cpp`). `false` with no routing data, unknown race/class, or no admitting
    /// row — a missing button beats offering an unlearn the server would anticheat-flag.
    pub fn abandonable(&self, line_id: u32, race: u8, class: u8) -> bool {
        self.race_class(line_id, race, class)
            .is_some_and(SkillRaceClass::unlearnable)
    }

    /// The `SkillRaceClassInfo` row the client resolves for `line_id` × `race`/`class` (1-based
    /// unit bytes) — [`SkillRaceClass`], the whole of what the Skills tab's display law reads.
    /// `None` when no row admits this character: the real client's list build drops such a line
    /// outright (`0x4d2cb0`'s `!srci → continue`), so a caller building the pane must too.
    pub fn race_class(&self, line_id: u32, race: u8, class: u8) -> Option<SkillRaceClass> {
        self.srci_row(line_id, race, class).map(|r| r.row)
    }

    /// Whether `line_id` is a **single-rank** line for `race`/`class` (1-based unit bytes): the
    /// admitting `SkillRaceClassInfo` row carries [`SKILL_FLAG_MONO_VALUE`] (`0x400`), so the
    /// client's `GetSkillLineInfo` reports its `skillMaxRank` as `1` no matter what the server's
    /// descriptor holds — and the skills pane draws it as a proficiency (gray bar, no rank text).
    /// `false` with no routing data, unknown race/class, or no admitting row: a line we can't
    /// classify keeps the server's own numbers rather than being silently blanked.
    pub fn mono_value(&self, line_id: u32, race: u8, class: u8) -> bool {
        self.race_class(line_id, race, class)
            .is_some_and(SkillRaceClass::mono)
    }

    /// Whether a rank change in `line_id` prints the skill-up / skill-gained chat line for a
    /// character of `race`/`class` (1-based unit bytes) — [`SkillRaceClass::skill_up_silent`],
    /// inverted. `false` with no admitting row: the real watcher skips the message when its
    /// `SkillRaceClassInfo` resolve comes back empty too (`0x5de352 je`, the same taken branch as
    /// the flag test), so a line this character can't legally
    /// hold stays silent however it got into the block.
    pub fn announces_skill_ups(&self, line_id: u32, race: u8, class: u8) -> bool {
        self.race_class(line_id, race, class)
            .is_some_and(|rc| !rc.skill_up_silent())
    }

    /// A skill line's display (name + tab icon), by id.
    pub fn line(&self, line_id: u32) -> Option<&SkillLineInfo> {
        self.lines.get(&line_id)
    }

    /// A `SkillLineCategory.dbc` row's `(name, displayOrder)` — the skills pane's header for a
    /// line's [`SkillLineInfo::category_id`]; `None` for 0/unknown.
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

/// Load `SkillLineCategory.dbc` — id → (name, displayOrder). Missing/unparseable degrades to an
/// empty map (the skills pane then renders one flat group).
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

/// The `SkillRaceClassInfo.dbc` rows keyed by skill line — the General-collapse routing table. A
/// missing/unparseable file returns an empty map (the caller degrades to "each line its own tab").
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
            // First row wins (module doc): deterministic by file order. 44 of the 4753 spells
            // carry more than one row (race/class variants) — every spell probed by the tests
            // below has exactly one.
            let slot = abilities.entry(spell_id).or_insert(SlaInfo {
                skill_id,
                req_skill_value: u32_at(r, COL_SLA_REQ_SKILL_VALUE).unwrap_or(0),
                forward_spell_id,
                trivial_low: u32_at(r, COL_SLA_TRIVIAL_LOW).unwrap_or(0),
                trivial_high: u32_at(r, COL_SLA_TRIVIAL_HIGH).unwrap_or(0),
            });
            // …except the rank link, which takes the first NON-ZERO across the spell's rows —
            // vmangos's `GetSpellBookSuccessorSpellId` scans all of them (module doc).
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
