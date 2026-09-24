//! Unit reaction: the `FactionTemplate.dbc` catalog and the 1.12 client's reaction comparator.
//!
//! A unit's `UNIT_FIELD_FACTIONTEMPLATE` indexes `FactionTemplate.dbc`, and the reference's
//! comparator `0x606640` turns two rows into hostile, neutral or friendly; every `UnitReaction`,
//! `CanAttack` and `CanAssist` bottoms out in it. It is only the DBC half of `UnitReaction`
//! (`0x6061e0`), whose group, charm, PvP-flag and sanctuary overrides need session state.
//!
//! Also `Faction.dbc`'s reputation identity: the reference checks `FactionHasReputation` before the
//! comparator (`0x606530`), so a reputation faction's NPCs colour by the player's rank.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const FACTION_TEMPLATE: &str = "DBFilesClient\\FactionTemplate.dbc";
const FACTION: &str = "DBFilesClient\\Faction.dbc";
const FACTION_GROUP: &str = "DBFilesClient\\FactionGroup.dbc";

/// One `FactionTemplate.dbc` row: the fields the reaction comparator reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactionTemplate {
    /// The row's `Faction.dbc` id, what another template's `enemies`/`friends` lists name.
    pub faction: u32,
    /// The faction-group bits this template belongs to (Player/Alliance/Horde/Monster).
    pub group_mask: u32,
    pub friend_group_mask: u32,
    pub enemy_group_mask: u32,
    /// Explicit enemy `Faction.dbc` ids, 0-terminated.
    pub enemies: [u32; 4],
    /// Explicit friend `Faction.dbc` ids, 0-terminated.
    pub friends: [u32; 4],
}

/// A unit's base reaction toward another on the reference's scale: `0x606640` returns these three,
/// and the reputation path widens the scale to 0..7, hence the gaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reaction {
    Hostile = 1,
    Neutral = 3,
    Friendly = 4,
}

impl FactionTemplate {
    /// `self`'s reaction toward `other`, branch for branch `0x606640`. Hostility is only ever
    /// `self`'s, so colour by the unit's reaction toward the player, as the nameplate resolver
    /// `0x7cbaa0` does: a player template is enemy-masked against the whole Monster group.
    pub fn reaction_toward(&self, other: &FactionTemplate) -> Reaction {
        if self.enemy_group_mask & other.group_mask != 0 {
            return Reaction::Hostile;
        }
        for &enemy in &self.enemies {
            if enemy == 0 {
                break;
            }
            if enemy == other.faction {
                return Reaction::Hostile;
            }
        }
        if self.friend_group_mask & other.group_mask != 0 {
            return Reaction::Friendly;
        }
        for &friend in &self.friends {
            if friend == 0 {
                break;
            }
            if friend == other.faction {
                return Reaction::Friendly;
            }
        }
        if other.friend_group_mask & self.group_mask != 0 {
            return Reaction::Friendly;
        }
        for &friend in &other.friends {
            if friend == 0 {
                break;
            }
            if friend == self.faction {
                return Reaction::Friendly;
            }
        }
        Reaction::Neutral
    }
}

/// One `Faction.dbc` row's reputation identity: its list slot and the race/class-gated base values
/// the reference adds to the wire standing.
#[derive(Debug, Clone, Copy)]
pub struct FactionInfo {
    /// `reputationIndex`, the slot in the `SMSG_INITIALIZE_FACTIONS` standings, or -1 for none
    /// (`FactionHasReputation` tests `[faction+4] >= 0`).
    pub rep_index: i32,
    /// `Faction.dbc` field 18 (`team` in mangos): the parent faction, 0 for none, the only tree
    /// edge the 1.12 table has and the reputation pane's grouping key. A row is a header by
    /// [`faction_flags::HEADER`], not by being a parent.
    pub team: u32,
    race_masks: [u32; 4],
    class_masks: [u32; 4],
    base: [i32; 4],
    /// Per-slot `ReputationFlags`; `0x4` on the matched slot hides the unit tooltip's faction line.
    flags: [u32; 4],
}

impl FactionInfo {
    /// The slot the reference (`0x4d5500`..`0x4d5564`) picks for a `race`/`class`, both 1-based.
    /// `None` also keeps the faction out of the reputation pane, whose list build adds a faction
    /// only in this loop's accept block (`0x4d5555`). Unlike vmangos's `GetIndexFitTo`, a slot
    /// with both masks zero is rejected (`0x4d550b`), and the last matching slot wins.
    pub fn slot_for(&self, race: u8, class: u8) -> Option<usize> {
        let race_mask = 1u32 << (race.max(1) - 1);
        let class_mask = 1u32 << (class.max(1) - 1);
        (0..4).rfind(|&i| {
            let (races, classes) = (self.race_masks[i], self.class_masks[i]);
            (races != 0 || classes != 0)
                && (races == 0 || races & race_mask != 0)
                && (classes == 0 || classes & class_mask != 0)
        })
    }

    /// The base reputation for a player of `race`/`class`, 0 if no slot fits. The wire standing
    /// excludes it: a rank is taken from `wire + base` (`0x4d6370`).
    pub fn base_for(&self, race: u8, class: u8) -> i32 {
        self.slot_for(race, class).map_or(0, |i| self.base[i])
    }

    /// The `ReputationFlags` column for a player of `race`/`class`, 0 if no slot fits: the server's
    /// seed for a new character's flag byte (vmangos `ReputationMgr::GetDefaultStateFlags`). The
    /// reference never reads this column; the byte it displays is the wire's.
    pub fn default_flags_for(&self, race: u8, class: u8) -> u32 {
        self.slot_for(race, class).map_or(0, |i| self.flags[i])
    }

    /// Whether the unit tooltip shows this faction's name to a player of `race`/`class`: the unit
    /// builder `0x529fe0` takes the first matching slot, not the last, and shows the line unless
    /// its flags carry `0x4`. A zero race mask matches on the class mask instead.
    pub fn tooltip_shows_for(&self, race: u8, class: u8) -> bool {
        let race_bit = 1u32 << (race.max(1) - 1);
        let class_bit = 1u32 << (class.max(1) - 1);
        for i in 0..4 {
            let race_match = if self.race_masks[i] == 0 {
                self.class_masks[i]
            } else {
                self.race_masks[i] & race_bit
            };
            if race_match != 0 && (self.class_masks[i] == 0 || self.class_masks[i] & class_bit != 0)
            {
                return self.flags[i] & 0x4 == 0;
            }
        }
        false
    }
}

/// Total standing (base + wire) to rank 0..=7, hated to exalted, which is also the reference's
/// extended reaction scale. Widths from −42000: 36000, 3000, 3000, 3000, 6000, 12000, 21000, 1000
/// (vmangos `PointsInRank`).
pub fn reputation_rank(total_standing: i32) -> u8 {
    match total_standing {
        i32::MIN..=-6001 => 0, // hated
        -6000..=-3001 => 1,    // hostile
        -3000..=-1 => 2,       // unfriendly
        0..=2999 => 3,         // neutral
        3000..=8999 => 4,      // friendly
        9000..=20999 => 5,     // honored
        21000..=41999 => 6,    // revered
        42000..=i32::MAX => 7, // exalted
    }
}

/// The per-faction flag byte, the first field of each `SMSG_INITIALIZE_FACTIONS` entry, named from
/// the reference's own reads of its store (`0xb73294`), not from the emulators' enum. The reference
/// writes [`AT_WAR`] and [`INACTIVE`] itself, optimistically, since neither send is acked.
pub mod faction_flags {
    /// Listed in the reputation pane, the only flag that gates membership; the server sets it on
    /// first contact and sends `SMSG_SET_FACTION_VISIBLE`.
    pub const VISIBLE: u8 = 0x01;
    /// The player has declared war on this faction.
    pub const AT_WAR: u8 = 0x02;
    /// Suppresses the standing-change auto-reveal and rank-change chat line; the row stays listed.
    pub const HIDDEN: u8 = 0x04;
    /// The row is a header (`(flags >> 3) & 1`, `0x4d5acb`), a bit the emulators name
    /// `INVISIBLE_FORCED`. The five factions carrying it are the pane's header rows.
    pub const HEADER: u8 = 0x08;
    /// Overrides [`AT_WAR`]: war can never be declared; your own side's cities carry it.
    pub const PEACE_FORCED: u8 = 0x10;
    /// In the pane's inactive bucket, under the synthetic "Inactive" header rather than hidden.
    pub const INACTIVE: u8 = 0x20;
    /// One of a rival pair; the 1.12 client never tests this bit.
    pub const RIVAL: u8 = 0x40;
}

impl Reaction {
    /// A rank (0..=7) collapsed to three, as the nameplate palette `0x7cbaa0` does. The selection
    /// ring's palette (`0x605960`) keys the raw rank instead, with its own orange at 2.
    pub fn from_rank(rank: u8) -> Reaction {
        match rank {
            0..=1 => Reaction::Hostile,
            2..=3 => Reaction::Neutral,
            _ => Reaction::Friendly,
        }
    }
}

/// `FactionTemplate.dbc`, `Faction.dbc` and `FactionGroup.dbc` as id-to-row maps.
pub struct FactionCatalog {
    templates: HashMap<u32, FactionTemplate>,
    factions: HashMap<u32, FactionInfo>,
    /// `1 << MaskID` to the localized group name, `GetZonePVPInfo`'s territory line (`0x48d540`).
    group_names: HashMap<u32, String>,
    /// The same key to `InternalName` (field 2), English on every locale: `UnitFactionGroup`'s
    /// first return, which stock FrameXML concatenates into texture paths (`PlayerFrame.lua:68`).
    group_internal_names: HashMap<u32, String>,
    /// Faction id to localized name, for the item tooltip's "Requires <Faction> - <Standing>".
    names: HashMap<u32, String>,
    /// Faction id to localized description, the reputation pane's detail paragraph; most have none.
    descriptions: HashMap<u32, String>,
}

impl FactionCatalog {
    /// The template row for a `UNIT_FIELD_FACTIONTEMPLATE` id.
    pub fn template(&self, id: u32) -> Option<&FactionTemplate> {
        self.templates.get(&id)
    }

    /// A `Faction.dbc` id's reputation identity, `Some` only with a reputation slot: the
    /// reference's `FactionHasReputation` (`0x605fc0`), tested before the comparator (`0x606530`).
    pub fn reputation_faction(&self, faction_id: u32) -> Option<&FactionInfo> {
        self.factions.get(&faction_id).filter(|f| f.rep_index >= 0)
    }

    /// The localized name of a `Faction.dbc` id.
    pub fn faction_name(&self, faction_id: u32) -> Option<&str> {
        self.names.get(&faction_id).map(String::as_str)
    }

    /// The localized description of a `Faction.dbc` id, the reputation pane's detail paragraph.
    pub fn faction_description(&self, faction_id: u32) -> Option<&str> {
        self.descriptions.get(&faction_id).map(String::as_str)
    }

    /// Every faction with a reputation slot, with its identity.
    pub fn reputation_factions(&self) -> impl Iterator<Item = (u32, &FactionInfo)> {
        self.factions
            .iter()
            .filter(|(_, f)| f.rep_index >= 0)
            .map(|(&id, f)| (id, f))
    }

    /// The English `InternalName` of the first group bit in `mask`: `UnitFactionGroup`'s first
    /// return, which callers build texture paths from. [`Self::faction_group_name`] is the
    /// localized second return and the `GetZonePVPInfo` territory name.
    pub fn faction_group_internal_name(&self, mask: u32) -> Option<&str> {
        (0..32)
            .map(|b| 1u32 << b)
            .filter(|bit| mask & bit != 0)
            .find_map(|bit| self.group_internal_names.get(&bit))
            .map(String::as_str)
    }

    pub fn faction_group_name(&self, mask: u32) -> Option<&str> {
        (0..32)
            .map(|b| 1u32 << b)
            .filter(|bit| mask & bit != 0)
            .find_map(|bit| self.group_names.get(&bit))
            .map(String::as_str)
    }

    /// Number of template rows.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

/// `FactionTemplate.dbc`: 14 fields, 56-byte rows at the offsets `0x606640` reads (vmangos
/// `FactionTemplateEntry`).
fn faction_template_schema() -> Schema {
    let mut s = Schema::new("FactionTemplate");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("Faction", FieldType::UInt32),
        ("Flags", FieldType::UInt32),
        ("FactionGroup", FieldType::UInt32),
        ("FriendGroup", FieldType::UInt32),
        ("EnemyGroup", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    for i in 0..4 {
        s.add_field(SchemaField::new(format!("Enemy{i}"), FieldType::UInt32));
    }
    for i in 0..4 {
        s.add_field(SchemaField::new(format!("Friend{i}"), FieldType::UInt32));
    }
    s
}

/// `Faction.dbc`: 37 fields in build 5875 (vmangos `FactionEntry`).
fn faction_schema() -> Schema {
    let mut s = Schema::new("Faction");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("ReputationIndex", FieldType::Int32));
    for i in 0..4 {
        s.add_field(SchemaField::new(format!("RaceMask{i}"), FieldType::UInt32));
    }
    for i in 0..4 {
        s.add_field(SchemaField::new(format!("ClassMask{i}"), FieldType::UInt32));
    }
    for i in 0..4 {
        s.add_field(SchemaField::new(format!("Base{i}"), FieldType::Int32));
    }
    for i in 0..4 {
        s.add_field(SchemaField::new(format!("RepFlags{i}"), FieldType::UInt32));
    }
    s.add_field(SchemaField::new("Team", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Desc{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("DescFlags", FieldType::UInt32));
    s
}

/// `FactionGroup.dbc`: 12 fields, 48-byte rows; `MaskID` is a bit index, read by `0x48d540` at
/// `+0x4` with the name at `+0xc`.
fn faction_group_schema() -> Schema {
    let mut s = Schema::new("FactionGroup");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("MaskID", FieldType::UInt32));
    s.add_field(SchemaField::new("InternalName", FieldType::String));
    for i in 0..8 {
        s.add_field(SchemaField::new(format!("Name{i}"), FieldType::String));
    }
    s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
    s
}

/// Load the three faction tables off the patch chain.
pub fn load_faction_catalog(chain: &mut Chain) -> Result<FactionCatalog> {
    let bytes = chain
        .read_file(FACTION_TEMPLATE)
        .with_context(|| format!("reading {FACTION_TEMPLATE}"))?;
    let rs = parse(&bytes, faction_template_schema(), "FactionTemplate")?;
    let mut templates = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            let at = |i| u32_at(r, i).unwrap_or(0);
            templates.insert(
                id,
                FactionTemplate {
                    faction: at(1),
                    group_mask: at(3),
                    friend_group_mask: at(4),
                    enemy_group_mask: at(5),
                    enemies: [at(6), at(7), at(8), at(9)],
                    friends: [at(10), at(11), at(12), at(13)],
                },
            );
        }
    }

    let bytes = chain
        .read_file(FACTION)
        .with_context(|| format!("reading {FACTION}"))?;
    let rs = parse(&bytes, faction_schema(), "Faction")?;
    let mut factions = HashMap::with_capacity(rs.records().len());
    let mut names = HashMap::with_capacity(rs.records().len());
    let mut descriptions = HashMap::new();
    for r in rs.records() {
        if let Some(id) = u32_at(r, 0) {
            let at = |i| u32_at(r, i).unwrap_or(0);
            let iat = |i| u32_at(r, i).unwrap_or(0) as i32;
            factions.insert(
                id,
                FactionInfo {
                    rep_index: iat(1),
                    team: at(18),
                    race_masks: [at(2), at(3), at(4), at(5)],
                    class_masks: [at(6), at(7), at(8), at(9)],
                    base: [iat(10), iat(11), iat(12), iat(13)],
                    flags: [at(14), at(15), at(16), at(17)],
                },
            );
            // Name0 (enUS), column 19: after the ID, the index, 16 slot columns and `Team`.
            if let Some(name) = crate::dbc::str_at(&rs, r, 19) {
                names.insert(id, name);
            }
            // Desc0 (enUS), column 28, past the name block; empty on most rows.
            if let Some(desc) = crate::dbc::str_at(&rs, r, 28) {
                descriptions.insert(id, desc);
            }
        }
    }

    let bytes = chain
        .read_file(FACTION_GROUP)
        .with_context(|| format!("reading {FACTION_GROUP}"))?;
    let rs = parse(&bytes, faction_group_schema(), "FactionGroup")?;
    let mut group_names = HashMap::with_capacity(rs.records().len());
    let mut group_internal_names = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(mask_id), Some(name)) = (u32_at(r, 1), crate::dbc::str_at(&rs, r, 3)) else {
            continue;
        };
        // Field 2 is `InternalName`, field 3 `Name0`; `UnitFactionGroup` returns both.
        if let Some(internal) = crate::dbc::str_at(&rs, r, 2) {
            group_internal_names.insert(1u32 << mask_id, internal);
        }
        group_names.insert(1u32 << mask_id, name);
    }

    Ok(FactionCatalog {
        templates,
        factions,
        group_names,
        group_internal_names,
        names,
        descriptions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template with everything zeroed except what a test sets.
    fn tpl(faction: u32) -> FactionTemplate {
        FactionTemplate {
            faction,
            group_mask: 0,
            friend_group_mask: 0,
            enemy_group_mask: 0,
            enemies: [0; 4],
            friends: [0; 4],
        }
    }

    /// Every branch, in the order `0x606640` tests them, and enemy-beats-friend precedence.
    #[test]
    fn comparator_branch_domain() {
        let b = FactionTemplate {
            group_mask: 0b0010,
            ..tpl(50)
        };

        let a = FactionTemplate {
            enemy_group_mask: 0b0010,
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Hostile);

        // A.enemies[i] == B.faction → hostile, found in the last of the four slots.
        let a = FactionTemplate {
            enemies: [7, 8, 9, 50],
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Hostile);

        // The enemies list is 0-terminated: a match after a 0 is never reached.
        let a = FactionTemplate {
            enemies: [7, 0, 50, 0],
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Neutral);

        let a = FactionTemplate {
            friend_group_mask: 0b0010,
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Friendly);

        let a = FactionTemplate {
            friends: [50, 0, 0, 0],
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Friendly);

        // Friendship is read both ways: B's friend mask covers A's group.
        let a = FactionTemplate {
            group_mask: 0b1000,
            ..tpl(1)
        };
        let b_friendly = FactionTemplate {
            friend_group_mask: 0b1000,
            ..b
        };
        assert_eq!(a.reaction_toward(&b_friendly), Reaction::Friendly);

        let b_names_a = FactionTemplate {
            friends: [1, 0, 0, 0],
            ..b
        };
        assert_eq!(tpl(1).reaction_toward(&b_names_a), Reaction::Friendly);

        assert_eq!(tpl(1).reaction_toward(&b), Reaction::Neutral);

        // Hostility is tested first: an enemy-mask hit beats any friendship.
        let a = FactionTemplate {
            enemy_group_mask: 0b0010,
            friend_group_mask: 0b0010,
            friends: [50, 0, 0, 0],
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b_names_a), Reaction::Hostile);
    }

    /// Unit templates toward a human player (template 1), the direction `0x7cbaa0` colours by;
    /// ids from vmangos `creature_template.faction`, expected colours from a reference capture.
    #[test]
    fn real_dbc_reactions_match_reference_capture() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_faction_catalog(&mut chain).expect("load faction catalog");
        assert!(
            cat.len() > 200,
            "FactionTemplate loaded ({} rows)",
            cat.len()
        );

        let human_player = cat.template(1).expect("human player template (1)");
        let tpl = |id: u32| {
            cat.template(id)
                .unwrap_or_else(|| panic!("faction template {id}"))
        };
        let reaction = |id: u32| tpl(id).reaction_toward(human_player);
        assert_eq!(
            reaction(12),
            Reaction::Friendly,
            "Marshal Dughan (Stormwind)"
        );
        assert_eq!(reaction(31), Reaction::Neutral, "Chicken (critter)");
        assert_eq!(reaction(17), Reaction::Hostile, "Defias Trapper");
        assert_eq!(
            reaction(26),
            Reaction::Hostile,
            "Kobold Miner (aggro mine kobold)"
        );
        assert_eq!(
            reaction(25),
            Reaction::Neutral,
            "Kobold Vermin (non-aggro starter kobold)"
        );
        // The player is hostile toward the vermin (attackable) while the vermin is neutral to it.
        assert_eq!(human_player.reaction_toward(tpl(25)), Reaction::Hostile);

        // Stormwind (faction 72) has slot 19 and a human base of 4000, friendly before any
        // template comparison; the Chicken's faction 28 has no slot.
        let stormwind = cat
            .reputation_faction(72)
            .expect("Stormwind (72) has reputation");
        assert_eq!(stormwind.rep_index, 19);
        assert_eq!(stormwind.base_for(1, 1), 4000, "human base with Stormwind");
        assert_eq!(reputation_rank(stormwind.base_for(1, 1)), 4, "friendly");
        assert!(cat.reputation_faction(28).is_none(), "critter: no rep");
        // Orc (race 2) base with Stormwind is hated (−42000): rank 0, hostile.
        assert_eq!(stormwind.base_for(2, 1), -42000);
        assert_eq!(reputation_rank(stormwind.base_for(2, 1)), 0);

        // The unit tooltip (`0x529fe0`) shows "Stormwind" under a Stormwind guard to a human.
        assert_eq!(cat.faction_name(72), Some("Stormwind"));
        assert!(
            stormwind.tooltip_shows_for(1, 1),
            "Stormwind's matched slot is not rep-hidden for a human"
        );
        // Players never get the line: player factions have no reputation slot.
        assert!(
            cat.reputation_faction(tpl(1).faction).is_none(),
            "the human player faction has no reputation slot"
        );
    }

    #[test]
    fn reputation_rank_thresholds() {
        for (standing, rank) in [
            (-42000, 0),
            (-6001, 0),
            (-6000, 1),
            (-3001, 1),
            (-3000, 2),
            (-1, 2),
            (0, 3),
            (2999, 3),
            (3000, 4),
            (8999, 4),
            (9000, 5),
            (20999, 5),
            (21000, 6),
            (41999, 6),
            (42000, 7),
            (42999, 7),
        ] {
            assert_eq!(reputation_rank(standing), rank, "standing {standing}");
        }
        // The nameplate's collapse: <= 1 hostile, >= 4 friendly, else neutral.
        assert_eq!(Reaction::from_rank(0), Reaction::Hostile);
        assert_eq!(Reaction::from_rank(1), Reaction::Hostile);
        assert_eq!(Reaction::from_rank(2), Reaction::Neutral);
        assert_eq!(Reaction::from_rank(3), Reaction::Neutral);
        assert_eq!(Reaction::from_rank(4), Reaction::Friendly);
        assert_eq!(Reaction::from_rank(7), Reaction::Friendly);
    }

    #[test]
    fn real_gameobject_factions_resolve_toward_both_player_templates() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_faction_catalog(&mut chain).expect("load factions");
        let (alliance, horde) = (
            cat.template(1).expect("Alliance player template"),
            cat.template(2).expect("Horde player template"),
        );

        // 114 "monster" (Deadmines' Factory Door): hostile to both, so no one can highlight it.
        let monster = cat.template(114).expect("FactionTemplate 114");
        assert_eq!(monster.reaction_toward(alliance), Reaction::Hostile);
        assert_eq!(monster.reaction_toward(horde), Reaction::Hostile);

        // 35 (levers, torches, Arathi Basin's banners): friendly to both, never gated.
        let usable = cat.template(35).expect("FactionTemplate 35");
        assert_eq!(usable.reaction_toward(alliance), Reaction::Friendly);
        assert_eq!(usable.reaction_toward(horde), Reaction::Friendly);
    }

    #[test]
    fn real_faction_group_names_by_mask() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_faction_catalog(&mut chain).expect("load factions");
        assert_eq!(cat.faction_group_name(2), Some("Alliance"));
        assert_eq!(cat.faction_group_name(4), Some("Horde"));
        assert_eq!(cat.faction_group_name(0), None);
    }

    #[test]
    fn real_reputation_factions_group_under_their_team() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_faction_catalog(&mut chain).expect("load factions");

        // 54 of the table's 190 rows carry a reputation slot; the rest are reaction-only.
        assert_eq!(cat.reputation_factions().count(), 54);

        // The four Alliance cities sit under Alliance (469), the four Horde ones under Horde (67).
        for id in [72 /* Stormwind */, 47, 69, 54] {
            assert_eq!(
                cat.reputation_faction(id).unwrap().team,
                469,
                "faction {id}"
            );
        }
        for id in [76 /* Orgrimmar */, 81, 68, 530] {
            assert_eq!(cat.reputation_faction(id).unwrap().team, 67, "faction {id}");
        }
        // The Steamwheedle towns sit under the cartel (169), a header with its own slot (10).
        for id in [21 /* Booty Bay */, 369, 470, 577] {
            assert_eq!(
                cat.reputation_faction(id).unwrap().team,
                169,
                "faction {id}"
            );
        }
        assert_eq!(cat.reputation_faction(169).unwrap().rep_index, 10);
        for id in [469, 67, 169] {
            assert_eq!(cat.reputation_faction(id).unwrap().team, 0, "faction {id}");
        }
        // Argent Dawn and the other ungrouped factions carry no team.
        assert_eq!(cat.reputation_faction(529).unwrap().team, 0);

        // Exactly five parents, each a pane header: Alliance, Horde, Steamwheedle Cartel and the
        // two battleground blocs.
        let mut parents: Vec<u32> = cat
            .reputation_factions()
            .map(|(_, f)| f.team)
            .filter(|&t| t != 0)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        parents.sort_unstable();
        assert_eq!(parents, [67, 169, 469, 891, 892]);

        assert!(
            cat.faction_description(529)
                .is_some_and(|d| d.starts_with("An organization focused on protecting Azeroth")),
            "Argent Dawn's description: {:?}",
            cat.faction_description(529)
        );
    }
}
