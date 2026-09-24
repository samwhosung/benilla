//! Unit reaction: the FactionTemplate.dbc catalog + the client's faction-reaction comparator.
//!
//! A unit's `UNIT_FIELD_FACTIONTEMPLATE` indexes **FactionTemplate.dbc**; the real client decides
//! hostile/neutral/friendly by comparing two of those rows with one pure function — the reaction
//! comparator at `0x606640`. Every
//! `UnitReaction`/`CanAttack`/`CanAssist` predicate bottoms out in it, and it's what colours nameplates
//! and the ground selection ring (red/yellow/green). [`FactionTemplate::reaction_toward`] is that
//! function, branch-for-branch.
//!
//! Record layout verified by the offsets the comparator reads (0x38-byte rows = 14 × u32, matching
//! vmangos `FactionTemplateEntry`): id@0, faction@1 (Faction.dbc id), flags@2, factionGroupMask@3,
//! friendGroupMask@4, enemyGroupMask@5, enemies[4]@6..9, friends[4]@10..13 (both id lists
//! 0-terminated).
//!
//! Also here: the **reputation identity** side of the decode — Faction.dbc's reputation slot,
//! parent pointer and race/class-gated base values ([`FactionInfo`]), the standing→rank thresholds
//! ([`reputation_rank`]), and the names of the wire flag byte's bits ([`faction_flags`]). The client
//! checks `FactionHasReputation` *before* the comparator (`0x606530`): a reputation faction's NPCs
//! colour by the player's rank, not by templates. The live standings are session state
//! (`SMSG_INITIALIZE_FACTIONS`) and live with the net layer.
//!
//! Two consumers share that identity now — the reaction decode above, and the **reputation pane**
//! (decision 1258, whose display law is `benilla-ui`'s `script::reputation`). Where both need the
//! same number they take it from the same function here, so a rank the pane draws and a colour the
//! nameplate paints can never disagree.
//!
//! Deliberately **not** here: the rest of the real `UnitReaction 0x6061e0` orchestration —
//! same-object/group/guild shortcuts, charm resolution, PvP-flag + sanctuary overrides. Those need
//! player/session state, not DBC rows; they land with the systems that own it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const FACTION_TEMPLATE: &str = "DBFilesClient\\FactionTemplate.dbc";
const FACTION: &str = "DBFilesClient\\Faction.dbc";
const FACTION_GROUP: &str = "DBFilesClient\\FactionGroup.dbc";

/// One FactionTemplate.dbc row — exactly the fields the reaction comparator reads (plus `faction`,
/// which the id-list scans compare against).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactionTemplate {
    /// The row's `Faction.dbc` id — what another template's `enemies`/`friends` lists name.
    pub faction: u32,
    /// The faction-group bits this template belongs to (Player/Alliance/Horde/Monster).
    pub group_mask: u32,
    /// Faction groups this template is friendly toward.
    pub friend_group_mask: u32,
    /// Faction groups this template is hostile toward.
    pub enemy_group_mask: u32,
    /// Explicit enemy `Faction.dbc` ids, 0-terminated.
    pub enemies: [u32; 4],
    /// Explicit friend `Faction.dbc` ids, 0-terminated.
    pub friends: [u32; 4],
}

/// A unit's base reaction toward another, on the client's own scale (`0x606640` returns exactly
/// these three; the extended reputation path widens the scale to 0..7, hence the gaps). The
/// nameplate/ring palette thresholds on this scale: `<= 1` red, `>= 4` green, else yellow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reaction {
    Hostile = 1,
    Neutral = 3,
    Friendly = 4,
}

impl FactionTemplate {
    /// The real client's faction-template reaction comparator — `self`'s reaction toward `other`.
    ///
    /// Branch-for-branch `0x606640`: enemy
    /// group mask, then the enemies id list, then friendship *both ways* (self's friend mask/list
    /// toward other, then other's friend mask/list toward self), else neutral. Hostility is only
    /// ever **self's** — and that asymmetry is load-bearing: a player template is enemy-masked
    /// against the whole Monster group (why you *may attack* a yellow beast), while a starter-zone
    /// kobold's template carries no enemy data at all. Colour by the **unit's** reaction toward the
    /// player — the direction the nameplate resolver `0x7cbaa0` verifiably calls (`this` = the unit)
    /// — or every wild beast would wrongly read hostile red.
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

/// One Faction.dbc row's reputation identity: the reputation-list slot plus the race/class-gated
/// base values the client adds to the wire standing. Only rows with `rep_index >= 0` participate in
/// reputation (`FactionHasReputation`, the client's `[faction+4] >= 0` check).
#[derive(Debug, Clone, Copy)]
pub struct FactionInfo {
    /// `reputationIndex` — the slot into the `SMSG_INITIALIZE_FACTIONS` standings array; `-1` = this
    /// faction has no reputation (reaction falls through to the template comparator).
    pub rep_index: i32,
    /// `Faction.dbc` field 18 — `team` in mangos, `m_parentFactionID` in the later builds' WDB
    /// names. The **parent pointer**: Stormwind/Ironforge/Darnassus/Gnomeregan Exiles all carry 469
    /// (Alliance), the four Steamwheedle towns carry 169, the six battleground factions carry
    /// 891/892, and `0` means no parent. It is the only tree edge 1.12's `Faction.dbc` has.
    ///
    /// It is the reputation pane's **grouping key**, but *not* what makes a row a header — that is
    /// the wire flag byte's [`faction_flags::HEADER`] bit. The distinction matters because the
    /// client synthesizes a header for a group key that has no header row yet, and because two of
    /// the pane's headers (`0` "Other" and `-1` "Inactive") name no faction at all.
    pub team: u32,
    race_masks: [u32; 4],
    class_masks: [u32; 4],
    base: [i32; 4],
    /// Per-slot reputation flags — bit `0x4` (mangos `FACTION_FLAG_HIDDEN`) on the matched slot
    /// suppresses the unit tooltip's faction-name line.
    flags: [u32; 4],
}

impl FactionInfo {
    /// The race/class-gated slot the **client** picks for a player of `race`/`class` (1-based ids),
    /// or `None` if no slot fits.
    ///
    /// `None` is not merely "no base value": it is the reputation pane's **membership gate** — the
    /// client's list build calls its add inside this loop's accept block (`0x4d5555`), so a faction
    /// no slot fits is not in the player's list at all.
    ///
    /// This is `0x4d5500`–`0x4d5564`, and it differs from vmangos's `GetIndexFitTo`
    /// in **two** ways that this project got wrong first time by reading the emulator instead:
    ///
    /// 1. A slot with **both** masks zero is a **reject**, not a double wildcard (`0x4d550b`). Only
    ///    one of the two may be zero-as-wildcard.
    /// 2. The loop has **no early exit** — the **last** matching slot wins, not the first.
    ///
    /// On the shipped 1.12 `Faction.dbc` the distinction is invisible: over all 190 rows × 72 valid
    /// race/class pairs, zero of the 13,680 evaluations disagree with the first-match rule (no row
    /// has a both-zero slot with a non-zero base, and none has two matching slots with different
    /// bases). It is written the client's way regardless, because a rule that is right only for the
    /// data we happen to ship is not a rule — a patched DBC would diverge silently.
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

    /// The base reputation for a player of `race`/`class` — the [`Self::slot_for`] slot's value,
    /// `0` if none fits. This is the term the wire standing EXCLUDES: the total a rank is taken
    /// from is this plus the wire value (byte-verified at `0x4d6370`: `wire + base`, and each
    /// addend has exactly one writer image-wide).
    pub fn base_for(&self, race: u8, class: u8) -> i32 {
        self.slot_for(race, class).map_or(0, |i| self.base[i])
    }

    /// The DBC's **default** reputation flag byte for a player of `race`/`class` — the same slot
    /// pick as [`Self::base_for`], reading the `ReputationFlags` column instead of the base
    /// (vmangos `ReputationMgr::GetDefaultStateFlags`). `0` if no slot fits.
    ///
    /// **The client never reads this column** (a byte-verified negative — the flag byte it displays
    /// from is entirely the wire's). It is the *server's* seed for a fresh character's per-slot
    /// state, and it is read here only to say what a faction is by default: notably that exactly
    /// the pane's five header factions carry [`faction_flags::HEADER`].
    pub fn default_flags_for(&self, race: u8, class: u8) -> u32 {
        self.slot_for(race, class).map_or(0, |i| self.flags[i])
    }

    /// Whether the unit tooltip shows this faction's name to a player of `race`/`class` — the
    /// byte loop in the unit builder `0x529fe0`: the FIRST of the four reputation slots that
    /// matches decides, and it shows the line iff its flags lack the hidden bit `0x4`. The slot
    /// match differs from [`Self::base_for`]'s in one spot the bytes insist on: a zero race mask
    /// substitutes the CLASS mask as the match value, so a slot with both masks zero matches
    /// nothing here (while `base_for` treats it as a wildcard). No matching slot → no line.
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

/// Reputation rank thresholds (vanilla): total standing (base + wire) → rank `0..=7` (hated,
/// hostile, unfriendly, neutral, friendly, honored, revered, exalted). The rank scale **is** the
/// client's extended reaction scale — the ring/nameplate palette thresholds apply to it directly
/// (`<= 1` red, `>= 4` green, else yellow). Rank widths from the bottom (−42000): 36000, 3000,
/// 3000, 3000, 6000, 12000, 21000, 1000 (vmangos `PointsInRank`, matching the client).
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

/// The per-slot reputation flag byte the server keeps for each of the player's factions and sends
/// as the first field of every `SMSG_INITIALIZE_FACTIONS` entry.
///
/// **Named from the CLIENT's own reads, not from the emulators' enum** (the thirteen accesses to
/// the store at `0xb73294` are the complete image-wide population). That distinction is not
/// pedantry — the two disagree on bit `0x08`, which every emulator calls `INVISIBLE_FORCED` and
/// the client tests as **HEADER** (`entry.isHeader = (flags >> 3) & 1`, `0x4d5acb`). Believing the
/// emulator's name gets the *right rows* on screen for the wrong reason (the five factions carrying
/// it are exactly the pane's headers, so "hide these" and "these are headers" pick the same set)
/// and then gets the header machinery wrong everywhere it matters.
///
/// The same bit layout appears in `Faction.dbc`'s four `ReputationFlags` columns, which is where the
/// **server** seeds a slot's initial byte from (`ReputationMgr::GetDefaultStateFlags`). The client
/// never reads that column — the byte it displays from is entirely the wire's.
///
/// Two of these the client also *writes*, optimistically, because neither send is acked: [`AT_WAR`]
/// and [`INACTIVE`].
pub mod faction_flags {
    /// The faction is listed in the reputation pane at all. Off until the player first meets them;
    /// the server lifts it and pushes `SMSG_SET_FACTION_VISIBLE`. The **only** flag gating list
    /// membership.
    pub const VISIBLE: u8 = 0x01;
    /// The player has declared war on this faction.
    pub const AT_WAR: u8 = 0x02;
    /// Suppresses the auto-reveal on a standing change and the rank-change chat notification. It
    /// does **not** hide the row: a faction with this bit and [`VISIBLE`] is listed normally.
    pub const HIDDEN: u8 = 0x04;
    /// **This row is a header.** Not "force-invisible" — see the module doc; the client's own test
    /// is `(flags >> 3) & 1` at `0x4d5acb`. The five factions that carry it (Alliance, Horde,
    /// Steamwheedle Cartel, and the two battleground blocs) are the pane's header rows.
    pub const HEADER: u8 = 0x08;
    /// Overrides [`AT_WAR`]: war can never be declared, so the pane's box is disabled. Your own
    /// side's city factions carry it.
    pub const PEACE_FORCED: u8 = 0x10;
    /// The player moved this faction to the pane's inactive bucket — which re-parents the row under
    /// the synthetic "Inactive" header, rather than hiding it.
    pub const INACTIVE: u8 = 0x20;
    /// The two competing factions of a rival pair. **The 1.12 client never tests this bit** — it is
    /// here to name the byte completely, not because anything reads it.
    pub const RIVAL: u8 = 0x40;
}

impl Reaction {
    /// A reputation rank (`0..=7`, [`reputation_rank`]) collapsed to the three-way scale, the way
    /// the **nameplate** palette thresholds it (`0x7cbaa0`: `<= 1` hostile, `>= 4` friendly, else
    /// neutral). The selection **ring** does *not* collapse — its own palette (`0x605960`) keys the
    /// raw rank and has a distinct orange at rank 2 — so ring consumers use the rank directly.
    pub fn from_rank(rank: u8) -> Reaction {
        match rank {
            0..=1 => Reaction::Hostile,
            2..=3 => Reaction::Neutral,
            _ => Reaction::Friendly,
        }
    }
}

/// FactionTemplate.dbc + Faction.dbc + FactionGroup.dbc loaded into id → row maps (the template id
/// is what a unit's `UNIT_FIELD_FACTIONTEMPLATE` carries; the faction id is the template's
/// `faction` field; the group names key by mask bit).
pub struct FactionCatalog {
    templates: HashMap<u32, FactionTemplate>,
    factions: HashMap<u32, FactionInfo>,
    /// `1 << FactionGroup.MaskID` → the localized group Name ("Alliance", "Horde") — what
    /// `GetZonePVPInfo`'s territory line formats (`0x48d540` reads FactionGroup.dbc Name of the
    /// row whose bit ∈ the zone's FactionGroupMask).
    group_names: HashMap<u32, String>,
    /// The same key → FactionGroup.dbc's **`InternalName`** (field 2), which is English on every
    /// locale where `Name0` is not. Kept beside the localized map rather than derived from it
    /// because `UnitFactionGroup` returns BOTH and they are not interchangeable: its first return
    /// names a TEXTURE — stock builds `"Interface\TargetingFrame\UI-PVP-"..factionGroup`
    /// (`PlayerFrame.lua:68`, `TargetFrame.lua:198`, `PartyMemberFrame.lua:125`) and
    /// `"…\Battleground-"..UnitFactionGroup("player")` (`BattlefieldFrame.lua:195`) — so a
    /// localized string there is a path that does not exist. `HonorFrame.lua:68` compares it to
    /// the literal `"Alliance"` for the same reason.
    group_internal_names: HashMap<u32, String>,
    /// Faction id → the localized Name — what the item tooltip's "Requires <Faction> -
    /// <Standing>" line prints.
    names: HashMap<u32, String>,
    /// Faction id → the localized Description — the paragraph the reputation pane's detail popup
    /// prints under the faction name. Absent for most rows: only the pane's own factions carry one.
    descriptions: HashMap<u32, String>,
}

impl FactionCatalog {
    /// The template row for a `UNIT_FIELD_FACTIONTEMPLATE` id, or `None` for an id the DBC lacks.
    pub fn template(&self, id: u32) -> Option<&FactionTemplate> {
        self.templates.get(&id)
    }

    /// The reputation identity of a `Faction.dbc` id, `Some` only when the faction actually has a
    /// reputation slot — the client's `FactionHasReputation` (`0x605fc0`, `[faction+4] >= 0`). For
    /// these factions unit reaction is the **player's reputation rank**, checked *before* the
    /// template comparator (`0x606530`).
    pub fn reputation_faction(&self, faction_id: u32) -> Option<&FactionInfo> {
        self.factions.get(&faction_id).filter(|f| f.rep_index >= 0)
    }

    /// The localized name of a `Faction.dbc` id — the reputation-requirement line's faction.
    pub fn faction_name(&self, faction_id: u32) -> Option<&str> {
        self.names.get(&faction_id).map(String::as_str)
    }

    /// The localized description of a `Faction.dbc` id — the reputation pane's detail paragraph.
    /// `None` where the DBC leaves it empty, which is most non-pane factions.
    pub fn faction_description(&self, faction_id: u32) -> Option<&str> {
        self.descriptions.get(&faction_id).map(String::as_str)
    }

    /// Every faction that has a reputation slot, with its identity — the app builds the
    /// player's faction → rank map from this (base + wire standing, ranked).
    pub fn reputation_factions(&self) -> impl Iterator<Item = (u32, &FactionInfo)> {
        self.factions
            .iter()
            .filter(|(_, f)| f.rep_index >= 0)
            .map(|(&id, f)| (id, f))
    }

    /// The localized FactionGroup name of the (first) group bit set in `mask` — "Alliance" for 2,
    /// "Horde" for 4; `None` for an unowned mask. The `GetZonePVPInfo` territory-line lookup.
    /// The **English** group name for a mask — FactionGroup.dbc's `InternalName`. This is the one
    /// `UnitFactionGroup`'s FIRST return must carry, because callers concatenate it into a texture
    /// path; [`Self::faction_group_name`] is the localized twin for the second return.
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

    /// Number of template rows (for logging/diagnostics).
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// Whether no rows loaded.
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

/// FactionTemplate.dbc — 14 fields in build 5875 (`14 × 4 == 56`-byte records, the exact offsets the
/// client comparator reads; cross-checked with vmangos `FactionTemplateEntry`).
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

/// Faction.dbc — 37 fields in build 5875 (`ID`, `reputationIndex`, 4× race masks, 4× class masks,
/// 4× base values, 4× reputation flags, `team`, then 8+1 name and 8+1 description columns;
/// cross-checked with vmangos `FactionEntry` and the file header). We read the reputation identity.
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

/// FactionGroup.dbc — 12 fields in build 5875 (48-byte records: `ID`, `MaskID` (the bit index),
/// an internal name, then the 8+1 localized Name block; layout per the `GetZonePVPInfo` reader
/// `0x48d540` — MaskID@+0x4, Name@+0xc).
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

/// Load FactionTemplate.dbc + Faction.dbc from the patch chain into a [`FactionCatalog`].
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
            // Name0 (enUS) — column 19 after ID/RepIndex/4·race/4·class/4·base/4·flags/Team.
            if let Some(name) = crate::dbc::str_at(&rs, r, 19) {
                names.insert(id, name);
            }
            // Desc0 (enUS) — column 28, past the 8 name columns and their flag word. Empty for
            // most rows, so this map is far smaller than `names` and is left unsized.
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
        // Field 2 is `InternalName`, field 3 is `Name0` — see the schema above. Both are read
        // because `UnitFactionGroup` returns the pair and only the localized half was kept before.
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

    /// The comparator's full branch domain, in the byte order `0x606640` tests them: enemy mask,
    /// enemies list (with its 0-terminator + 4-entry bound), friend mask (A), friends list (A),
    /// friend mask (B toward A's group), friends list (B naming A), neutral fall-through — and
    /// enemy-beats-friend precedence.
    #[test]
    fn comparator_branch_domain() {
        let b = FactionTemplate {
            group_mask: 0b0010,
            ..tpl(50)
        };

        // (A.enemyGroupMask & B.group) != 0 → hostile.
        let a = FactionTemplate {
            enemy_group_mask: 0b0010,
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Hostile);

        // A.enemies[i] == B.faction → hostile (in the last slot: the loop scans all 4).
        let a = FactionTemplate {
            enemies: [7, 8, 9, 50],
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Hostile);

        // The enemies list is 0-terminated: a match *after* a 0 is never reached.
        let a = FactionTemplate {
            enemies: [7, 0, 50, 0],
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Neutral);

        // (A.friendGroupMask & B.group) != 0 → friendly.
        let a = FactionTemplate {
            friend_group_mask: 0b0010,
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Friendly);

        // A.friends[i] == B.faction → friendly.
        let a = FactionTemplate {
            friends: [50, 0, 0, 0],
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b), Reaction::Friendly);

        // (B.friendGroupMask & A.group) != 0 → friendly (friendship read both ways).
        let a = FactionTemplate {
            group_mask: 0b1000,
            ..tpl(1)
        };
        let b_friendly = FactionTemplate {
            friend_group_mask: 0b1000,
            ..b
        };
        assert_eq!(a.reaction_toward(&b_friendly), Reaction::Friendly);

        // B.friends[i] == A.faction → friendly.
        let b_names_a = FactionTemplate {
            friends: [1, 0, 0, 0],
            ..b
        };
        assert_eq!(tpl(1).reaction_toward(&b_names_a), Reaction::Friendly);

        // Nothing matches → neutral.
        assert_eq!(tpl(1).reaction_toward(&b), Reaction::Neutral);

        // Hostility is tested first: an enemy-mask hit wins over any friendship evidence.
        let a = FactionTemplate {
            enemy_group_mask: 0b0010,
            friend_group_mask: 0b0010,
            friends: [50, 0, 0, 0],
            ..tpl(1)
        };
        assert_eq!(a.reaction_toward(&b_names_a), Reaction::Hostile);
    }

    /// End-to-end on the **real** build-5875 DBC, in the direction the client colours by (the unit's
    /// reaction toward the player — verified in the binary: the nameplate resolver `0x7cbaa0` calls
    /// `unit->UnitReaction(activePlayer)`): a human player is faction template 1; Marshal Dughan (12)
    /// reads friendly and the Chicken (31) neutral (the exact ring colours a reference capture
    /// recorded), a Defias Trapper (17) and mine Kobold Miner (26) hostile, and the starter-field
    /// Kobold Vermin (25) **neutral** — the famous non-aggro yellow level-1 mob. That last one also
    /// locks the direction: the *player's* template is enemy-masked against the whole Monster group,
    /// so the reverse direction would wrongly read every yellow beast as hostile. Template ids are
    /// the vmangos `creature_template.faction` rows. Guards the schema (a shifted column scrambles
    /// every mask) and the comparator against ground truth.
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
        // The direction asymmetry itself: the player is hostile *toward* the vermin (attackable),
        // while the vermin — what the ring colours by — is neutral toward the player.
        assert_eq!(human_player.reaction_toward(tpl(25)), Reaction::Hostile);

        // The reputation identity: Stormwind (faction 72, both guard templates 11 + 12 point at it)
        // has reputation slot 19 with a human base of 4000 — friendly rank 4 at zero wire standing,
        // which is why every Stormwind NPC is green to a human **before** any template comparison
        // (even in GM mode — director-verified on the reference client). A critter faction (the
        // Chicken's 28) has no reputation slot → the comparator path.
        let stormwind = cat
            .reputation_faction(72)
            .expect("Stormwind (72) has reputation");
        assert_eq!(stormwind.rep_index, 19);
        assert_eq!(stormwind.base_for(1, 1), 4000, "human base with Stormwind");
        assert_eq!(reputation_rank(stormwind.base_for(1, 1)), 4, "friendly");
        assert!(cat.reputation_faction(28).is_none(), "critter: no rep");
        // Orc (race 2) base with Stormwind is hated (−42000): rank 0 → hostile red.
        assert_eq!(stormwind.base_for(2, 1), -42000);
        assert_eq!(reputation_rank(stormwind.base_for(2, 1)), 0);

        // The unit tooltip's faction-name line (the `0x529fe0` tail): a Stormwind guard shows
        // "Stormwind" to a human — the ref's Marshal McBride "Level 20 / Stormwind / PvP" shot.
        assert_eq!(cat.faction_name(72), Some("Stormwind"));
        assert!(
            stormwind.tooltip_shows_for(1, 1),
            "Stormwind's matched slot is not rep-hidden for a human"
        );
        // Players never get the line: the PLAYER,* factions carry no reputation slot, so the
        // builder's rep-index gate drops it before the slot walk.
        assert!(
            cat.reputation_faction(tpl(1).faction).is_none(),
            "the human player faction has no reputation slot"
        );
    }

    /// The rank thresholds (vanilla `PointsInRank`), exact at every boundary.
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
        // The palette mapping on the rank scale: <=1 red, >=4 green, else yellow.
        assert_eq!(Reaction::from_rank(0), Reaction::Hostile);
        assert_eq!(Reaction::from_rank(1), Reaction::Hostile);
        assert_eq!(Reaction::from_rank(2), Reaction::Neutral);
        assert_eq!(Reaction::from_rank(3), Reaction::Neutral);
        assert_eq!(Reaction::from_rank(4), Reaction::Friendly);
        assert_eq!(Reaction::from_rank(7), Reaction::Friendly);
    }

    /// The GameObject faction term (decision 0764), pinned on the real `FactionTemplate.dbc`: the
    /// factions that shipped GameObjects actually carry, resolved **GO → player** (the direction
    /// `0x606640` uses). A column slip here would silently blank ~7,563 shipped GO spawns, or fail
    /// to blank them — so the two decisive rows are asserted by value. Skips without client data.
    #[test]
    fn real_gameobject_factions_resolve_toward_both_player_templates() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_faction_catalog(&mut chain).expect("load factions");
        let (alliance, horde) = (
            cat.template(1).expect("Alliance player template"),
            cat.template(2).expect("Horde player template"),
        );

        // 114 "monster" — what Deadmines' Factory Door and Stratholme's scenery portcullises carry.
        // Hostile to BOTH sides, so `> 1` fails and the object is not highlightable for anyone.
        let monster = cat.template(114).expect("FactionTemplate 114");
        assert_eq!(monster.reaction_toward(alliance), Reaction::Hostile);
        assert_eq!(monster.reaction_toward(horde), Reaction::Hostile);

        // 35 — the levers/torches, and (the reason this matters) Arathi Basin's capture banners.
        // FRIENDLY to both, so the faction gate must never touch them.
        let usable = cat.template(35).expect("FactionTemplate 35");
        assert_eq!(usable.reaction_toward(alliance), Reaction::Friendly);
        assert_eq!(usable.reaction_toward(horde), Reaction::Friendly);
    }

    /// The real 5875 FactionGroup table: the territory-line names resolve by mask bit — Alliance
    /// mask 2, Horde mask 4, nothing for an unowned 0 (the `GetZonePVPInfo` lookup, `0x48d540`).
    /// Skips without client data.
    #[test]
    fn real_faction_group_names_by_mask() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_faction_catalog(&mut chain).expect("load factions");
        assert_eq!(cat.faction_group_name(2), Some("Alliance"));
        assert_eq!(cat.faction_group_name(4), Some("Horde"));
        assert_eq!(cat.faction_group_name(0), None);
    }

    /// **`team` is the reputation pane's tree, on the real table.** 1.12's `Faction.dbc` has no
    /// parent column — field 18 (`team`) is the only edge it carries — and every group the pane
    /// draws a header for is a set of rows sharing one `team`. Asserted by value on the real DBC
    /// because a one-column slip here would silently produce a flat list that still *renders*.
    ///
    /// The counts are the whole table's, so a future patch chain that changes them fails loudly
    /// rather than quietly regrouping the pane. Skips without client data.
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
        // The Steamwheedle towns sit under the cartel itself (169) — a header that is also a
        // reputation faction in its own right (slot 10), which is why "is a header" cannot be
        // "has no reputation slot".
        for id in [21 /* Booty Bay */, 369, 470, 577] {
            assert_eq!(
                cat.reputation_faction(id).unwrap().team,
                169,
                "faction {id}"
            );
        }
        assert_eq!(cat.reputation_faction(169).unwrap().rep_index, 10);
        // …and the parents themselves have no parent: they are the top of the tree.
        for id in [469, 67, 169] {
            assert_eq!(cat.reputation_faction(id).unwrap().team, 0, "faction {id}");
        }
        // The pane's leftovers — Argent Dawn and the rest — carry no team at all.
        assert_eq!(cat.reputation_faction(529).unwrap().team, 0);

        // Exactly five factions are named as a parent by somebody, and every one of them is a
        // header the pane draws: Alliance, Horde, Steamwheedle Cartel, and the two BG blocs.
        let mut parents: Vec<u32> = cat
            .reputation_factions()
            .map(|(_, f)| f.team)
            .filter(|&t| t != 0)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        parents.sort_unstable();
        assert_eq!(parents, [67, 169, 469, 891, 892]);

        // The detail popup's paragraph comes off the same row, past the eight name columns.
        assert!(
            cat.faction_description(529)
                .is_some_and(|d| d.starts_with("An organization focused on protecting Azeroth")),
            "Argent Dawn's description: {:?}",
            cat.faction_description(529)
        );
    }
}
